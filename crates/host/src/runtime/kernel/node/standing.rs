//! One node socket per program carries its changes and its standing queries (`program.watch`), and a
//! view hears every request it has on one feed through one joint, so one block lands in one redraw.
use super::*;
use noded_client::wire::{Outcome, Pushed, Watch};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Default)]
pub(super) struct Delivery {
    pub(super) change: Option<Answer>,
    answers: BTreeMap<u32, (u64, Watched)>,
}

#[derive(Clone)]
enum Watched {
    Answered(Vec<u8>),
    Moved,
    Refused(wire::Error),
    Ended(wire::Error),
}

impl Watched {
    fn of(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Answered(bytes) => Watched::Answered(bytes),
            Outcome::Moved => Watched::Moved,
            Outcome::Refused(refusal) => Watched::Refused(said(refusal)),
        }
    }

    fn item(self) -> (Answer, bool) {
        match self {
            Watched::Answered(bytes) => (Ok([&[1][..], &bytes[..]].concat()), false),
            Watched::Moved => (Ok(methods::encode(&None::<()>)), false),
            Watched::Refused(refusal) => (Err(refusal), false),
            Watched::Ended(refusal) => (Err(refusal), true),
        }
    }
}

fn said(refusal: abi::Refusal) -> wire::Error {
    wire::Error::new(refusal.reason, refusal.sentence)
}

pub(super) fn delivered(pending: Option<Delivery>, item: Delivery) -> Delivery {
    let Some(mut pending) = pending else {
        return item;
    };
    pending.change = match (pending.change.take(), item.change) {
        (Some(before), Some(change)) => Some(coalesced(Some(before), change)),
        (before, change) => before.or(change),
    };
    for (id, answer) in item.answers {
        if !matches!(pending.answers.get(&id), Some((_, Watched::Ended(_)))) {
            pending.answers.insert(id, answer);
        }
    }
    pending
}

#[derive(Default)]
pub(super) struct Standings {
    table: Mutex<Table>,
    moved: tokio::sync::Notify,
}

#[derive(Default)]
struct Table {
    ids: u32,
    told: u64,
    held: BTreeMap<u32, Standing>,
}

struct Standing {
    body: Vec<u8>,
    key: [u8; 32],
    signer: Arc<dyn noded_client::Signer>,
    network: String,
    holders: usize,
    last: Option<(u64, Watched)>,
    retried: bool,
}

impl Table {
    fn told(&mut self, id: u32, watched: Watched) -> Option<(u64, Watched)> {
        if !self.held.contains_key(&id) {
            return None;
        }
        self.told += 1;
        let told = (self.told, watched);
        if matches!(told.1, Watched::Ended(_)) {
            self.held.remove(&id);
        } else if let Some(standing) = self.held.get_mut(&id) {
            standing.last = Some(told.clone());
            standing.retried = false;
        }
        Some(told)
    }
}

impl Standings {
    fn hold(self: &Arc<Self>, body: &[u8], node: &Node) -> Held {
        let signer = node.signer();
        let key = signer.public_key();
        let mut table = lock(&self.table);
        let found = table
            .held
            .iter_mut()
            .find(|(_, standing)| standing.key == key && standing.body == body);
        if let Some((id, standing)) = found {
            standing.holders += 1;
            return Held {
                standings: self.clone(),
                id: *id,
            };
        }
        table.ids = table.ids.wrapping_add(1);
        let id = table.ids;
        let standing = Standing {
            body: body.to_vec(),
            key,
            signer,
            network: node.network.clone(),
            holders: 1,
            last: None,
            retried: false,
        };
        table.held.insert(id, standing);
        drop(table);
        self.moved.notify_one();
        Held {
            standings: self.clone(),
            id,
        }
    }

    fn last(&self, id: u32) -> Option<(u64, Watched)> {
        let table = lock(&self.table);
        table
            .held
            .get(&id)
            .and_then(|standing| standing.last.clone())
    }
}

struct Held {
    standings: Arc<Standings>,
    id: u32,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut table = lock(&self.standings.table);
        let Some(standing) = table.held.get_mut(&self.id) else {
            return;
        };
        standing.holders -= 1;
        if standing.holders == 0 {
            table.held.remove(&self.id);
            drop(table);
            self.standings.moved.notify_one();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) async fn socket(
    client: RpcClient,
    program: String,
    followers: Followers<Delivery>,
    standings: Arc<Standings>,
) {
    let pushing = Pushing {
        client: &client,
        program: &program,
        followers: &followers,
        standings: &standings,
    };
    pushing.run().await
}

struct Pushing<'a> {
    client: &'a RpcClient,
    program: &'a str,
    followers: &'a Followers<Delivery>,
    standings: &'a Standings,
}

impl Pushing<'_> {
    async fn run(&self) {
        let mut block_time_ms = None;
        let mut away = false;
        loop {
            let lost = match self.opened(&mut away, &mut block_time_ms).await {
                Ok(lost) => lost,
                Err(error) if node_refuses(&error) => {
                    tracing::debug!(target: "ducktape::app", %error, program = self.program,
                        "changes stream not opened");
                    return self.end(refused(error));
                }
                Err(error) => error.to_string(),
            };
            if !std::mem::replace(&mut away, true) {
                tracing::warn!(target: "ducktape::app", program = self.program, reason = lost,
                    "changes stream lost: it opens again once the node answers");
            }
            match self.client.answering(block_time_ms).await {
                Ok(answered) => block_time_ms = Some(answered),
                Err(error) => return self.end(refused(error)),
            }
        }
    }

    async fn opened(
        &self,
        away: &mut bool,
        block_time_ms: &mut Option<u64>,
    ) -> noded_client::Result<String> {
        match self.client.watch(self.program).await {
            Ok((watching, pushed)) => {
                self.reopened(away, block_time_ms).await;
                Ok(self.watched(&watching, pushed).await)
            }
            Err(noded_client::Error::Failed { status: 404, .. }) => {
                let changes = self.client.changed_keys(self.program).await?;
                self.reopened(away, block_time_ms).await;
                Ok(self.unwatched(changes).await)
            }
            Err(error) => Err(error),
        }
    }

    async fn reopened(&self, away: &mut bool, block_time_ms: &mut Option<u64>) {
        if std::mem::take(away) {
            tracing::info!(target: "ducktape::app", program = self.program, "changes stream open again");
            let reread = Delivery {
                change: Some(Ok(reread())),
                ..Delivery::default()
            };
            lock(self.followers).send(reread);
        }
        if block_time_ms.is_none() {
            *block_time_ms = self
                .client
                .status()
                .await
                .ok()
                .map(|status| status.block_time_ms);
        }
    }

    async fn watched(
        &self,
        watching: &noded_client::Watching,
        mut pushed: impl futures::Stream<Item = noded_client::Result<Pushed>> + Unpin,
    ) -> String {
        use futures::StreamExt as _;
        let mut sent = BTreeSet::new();
        loop {
            if !self.asked(watching, &mut sent).await {
                return "the socket closed".to_owned();
            }
            let pushed = tokio::select! {
                pushed = pushed.next() => pushed,
                () = self.standings.moved.notified() => continue,
            };
            match pushed {
                Some(Ok(pushed)) => self.heard(pushed, &mut sent).await,
                Some(Err(error)) => return error.to_string(),
                None => return "the socket closed".to_owned(),
            }
        }
    }

    async fn asked(&self, watching: &noded_client::Watching, sent: &mut BTreeSet<u32>) -> bool {
        let (asks, drops) = {
            let table = lock(&self.standings.table);
            let asks: Vec<_> = table
                .held
                .iter()
                .filter(|(id, _)| !sent.contains(*id))
                .map(|(id, standing)| {
                    let signer = standing.signer.clone();
                    (*id, standing.body.clone(), signer, standing.network.clone())
                })
                .collect();
            let drops: Vec<u32> = sent
                .iter()
                .filter(|id| !table.held.contains_key(*id))
                .copied()
                .collect();
            (asks, drops)
        };
        for id in drops {
            sent.remove(&id);
            if !watching.send(&Watch::Drop { id }) {
                return false;
            }
        }
        for (id, body, signer, network) in asks {
            let asked = self
                .client
                .ask_frame(&*signer, network.as_bytes(), self.program, body)
                .await;
            let frame = match asked {
                Ok(frame) => frame,
                Err(noded_client::Asked::Unsigned(unsigned)) => {
                    let locked = wire::Error::new(refusal::SESSION_LOCKED, unsigned.0);
                    self.deliver(None, [(id, Watched::Ended(locked))]);
                    continue;
                }
                Err(noded_client::Asked::Node(error)) => {
                    self.deliver(None, [(id, Watched::Ended(refused(error)))]);
                    continue;
                }
            };
            sent.insert(id);
            if !watching.send(&Watch::Ask { id, frame }) {
                return false;
            }
        }
        true
    }

    async fn heard(&self, pushed: Pushed, sent: &mut BTreeSet<u32>) {
        match pushed {
            Pushed::Answer {
                id,
                outcome: Outcome::Refused(refusal),
                ..
            } => {
                sent.remove(&id);
                let ask_again = refusal.reason == abi::reason::ANCHOR && self.may_ask_again(id);
                if ask_again {
                    let _ = self.client.status().await;
                    return;
                }
                self.deliver(None, [(id, Watched::Ended(said(refusal)))]);
            }
            Pushed::Answer { id, outcome, .. } => self.deliver(None, [(id, Watched::of(outcome))]),
            Pushed::Block {
                height,
                writes,
                answers,
                ..
            } => {
                let change = (!writes.is_empty()).then(|| {
                    let keys = writes.into_iter().map(|(key, _)| key).collect();
                    Ok(methods::encode(&Some(methods::Change { height, keys })))
                });
                let answers = answers
                    .into_iter()
                    .map(|(id, outcome)| (id, Watched::of(outcome)));
                self.deliver(change, answers);
            }
        }
    }

    fn may_ask_again(&self, id: u32) -> bool {
        let mut table = lock(&self.standings.table);
        let Some(standing) = table.held.get_mut(&id) else {
            return false;
        };
        !std::mem::replace(&mut standing.retried, true)
    }

    fn deliver(&self, change: Option<Answer>, outcomes: impl IntoIterator<Item = (u32, Watched)>) {
        let answers: BTreeMap<u32, (u64, Watched)> = {
            let mut table = lock(&self.standings.table);
            outcomes
                .into_iter()
                .filter_map(|(id, watched)| Some((id, table.told(id, watched)?)))
                .collect()
        };
        let anything = change.is_some() || !answers.is_empty();
        if anything {
            lock(self.followers).send(Delivery { change, answers });
        }
    }

    async fn unwatched(
        &self,
        mut changes: impl futures::Stream<Item = noded_client::Result<noded::Change>> + Unpin,
    ) -> String {
        use futures::StreamExt as _;
        loop {
            self.unsupported();
            let change = tokio::select! {
                change = changes.next() => change,
                () = self.standings.moved.notified() => continue,
            };
            let Some(Ok(change)) = change else {
                return "the socket closed".to_owned();
            };
            let keys = change.writes.into_iter().map(|(key, _)| key).collect();
            let change = methods::Change {
                height: change.height,
                keys,
            };
            let delivery = Delivery {
                change: Some(Ok(methods::encode(&Some(change)))),
                ..Delivery::default()
            };
            lock(self.followers).send(delivery);
        }
    }

    fn unsupported(&self) {
        let ids: Vec<u32> = lock(&self.standings.table).held.keys().copied().collect();
        let refusal = wire::Error::new(
            abi::reason::UNSUPPORTED,
            "this node pushes no query's answers: it has no /v1/watch",
        );
        let ended = ids
            .into_iter()
            .map(|id| (id, Watched::Ended(refusal.clone())));
        self.deliver(None, ended);
    }

    fn end(&self, refusal: wire::Error) {
        let answers = {
            let mut table = lock(&self.standings.table);
            let ids: Vec<u32> = table.held.keys().copied().collect();
            ids.into_iter()
                .filter_map(|id| Some((id, table.told(id, Watched::Ended(refusal.clone()))?)))
                .collect()
        };
        let mut fanout = lock(self.followers);
        fanout.ended = true;
        fanout.send(Delivery {
            change: Some(Err(refusal)),
            answers,
        });
    }
}

pub(in crate::runtime) struct Joint {
    joined: Arc<Joined>,
    task: rt::task::Task,
}

impl Drop for Joint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
struct Joined {
    members: Mutex<Vec<Member>>,
    poked: tokio::sync::Notify,
}

struct Member {
    key: u64,
    id: u64,
    claim: Claim,
    kind: Kind,
    _ended: tokio::sync::oneshot::Sender<()>,
}

enum Kind {
    Changes {
        reread: bool,
    },
    Watch {
        body: Vec<u8>,
        held: Option<Held>,
        told: u64,
    },
}

struct Membership {
    joint: Arc<Joint>,
    key: u64,
}

impl Drop for Membership {
    fn drop(&mut self) {
        lock(&self.joint.joined.members).retain(|member| member.key != self.key);
    }
}

pub(super) fn join(
    view: &mut View,
    id: u64,
    node: Node,
    program: String,
    call: Option<methods::Call>,
) {
    static KEYS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let key = KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (ended, gone) = tokio::sync::oneshot::channel();
    let claim = Claim::default();
    let (kind, follows) = match call {
        None => (
            Kind::Changes { reread: false },
            Follow::Changes(program.clone()),
        ),
        Some(call) => {
            let kind = Kind::Watch {
                body: call.body.clone(),
                held: None,
                told: 0,
            };
            (kind, Follow::Watch(call))
        }
    };
    let member = Member {
        key,
        id,
        claim: claim.clone(),
        kind,
        _ended: ended,
    };
    let place = (node.client.endpoint().to_owned(), program.clone());
    let joint = match view.joints.get(&place).and_then(std::sync::Weak::upgrade) {
        Some(joint) => {
            lock(&joint.joined.members).push(member);
            joint.joined.poked.notify_one();
            joint
        }
        None => {
            view.joints.retain(|_, joint| joint.strong_count() > 0);
            let joint = Arc::new(Joint::open(view, node, program, member));
            view.joints.insert(place, Arc::downgrade(&joint));
            joint
        }
    };
    let membership = Membership { joint, key };
    start(view, id, Some(follows), claim, async move {
        let _membership = membership;
        let _ = gone.await;
    });
}

impl Joint {
    fn open(view: &View, node: Node, program: String, first: Member) -> Self {
        let joined = Arc::new(Joined {
            members: Mutex::new(vec![first]),
            poked: tokio::sync::Notify::new(),
        });
        joined.poked.notify_one();
        let wanted = Wanted::of(view);
        let feed = wanted.now().then(|| changes_feed(&node.client, &program));
        let running = Running {
            joined: joined.clone(),
            replies: view.replies.clone(),
            node,
            program,
            wanted,
        };
        Joint {
            joined,
            task: rt::task::spawn(running.run(feed)),
        }
    }
}

struct Running {
    joined: Arc<Joined>,
    replies: Arc<Replies>,
    node: Node,
    program: String,
    wanted: Wanted,
}

enum Turn {
    Back,
    Away,
    Poked,
    Item(Delivery),
}

impl Running {
    async fn run(mut self, mut feed: Option<Changes>) {
        let mut drained = self.replies.drains();
        let mut after_an_end = false;
        loop {
            let turn = match &feed {
                None => {
                    self.wanted.until(true).await;
                    let idle = lock(&self.joined.members).is_empty();
                    if idle {
                        self.joined.poked.notified().await;
                        continue;
                    }
                    Turn::Back
                }
                Some((_, slot)) => tokio::select! {
                    delivery = slot.take() => Turn::Item(delivery),
                    () = self.joined.poked.notified() => Turn::Poked,
                    () = self.wanted.until(false) => Turn::Away,
                },
            };
            let delivery = match turn {
                Turn::Away => {
                    feed = None;
                    self.joined.away();
                    continue;
                }
                Turn::Back => {
                    feed = Some(changes_feed(&self.node.client, &self.program));
                    if !std::mem::take(&mut after_an_end) {
                        self.joined.back();
                    }
                    Delivery::default()
                }
                Turn::Poked => Delivery::default(),
                Turn::Item(delivery) => delivery,
            };
            let Some((joining, _)) = &feed else {
                continue;
            };
            let ended = matches!(delivery.change, Some(Err(_)));
            let standings = joining.shared.clone();
            let told = self.joined.tell(
                &self.replies,
                &mut drained,
                &self.node,
                &standings,
                delivery,
            );
            if ended {
                feed = None;
                after_an_end = true;
                self.joined.away();
                continue;
            }
            if !told {
                continue;
            }
            let away = tokio::select! {
                drawn = drained.changed() => {
                    if drawn.is_err() {
                        return;
                    }
                    false
                }
                () = self.wanted.until(false) => true,
            };
            if away {
                feed = None;
                self.joined.away();
            }
        }
    }
}

impl Joined {
    fn away(&self) {
        for member in lock(&self.members).iter_mut() {
            if let Kind::Watch { held, .. } = &mut member.kind {
                *held = None;
            }
        }
    }

    fn back(&self) {
        for member in lock(&self.members).iter_mut() {
            match &mut member.kind {
                Kind::Changes { reread } => *reread = true,
                Kind::Watch { told, .. } => *told = 0,
            }
        }
    }

    fn tell(
        &self,
        replies: &Replies,
        drained: &mut tokio::sync::watch::Receiver<()>,
        node: &Node,
        standings: &Arc<Standings>,
        delivery: Delivery,
    ) -> bool {
        let mut members = lock(&self.members);
        let mut items = Vec::new();
        for (at, member) in members.iter_mut().enumerate() {
            let item = match &mut member.kind {
                Kind::Changes { reread } => match std::mem::take(reread) {
                    true => Some((Ok(super::reread()), false)),
                    false => delivery.change.clone().map(|change| {
                        let done = change.is_err();
                        (change, done)
                    }),
                },
                Kind::Watch { body, held, told } => {
                    let held = held.get_or_insert_with(|| standings.hold(body, node));
                    let heard = match delivery.answers.get(&held.id) {
                        Some(answer) => Some(answer.clone()),
                        None if *told == 0 => standings.last(held.id),
                        None => None,
                    };
                    heard.filter(|(seq, _)| *seq > *told).map(|(seq, watched)| {
                        *told = seq;
                        watched.item()
                    })
                }
            };
            if let Some((answer, done)) = item {
                items.push((at, answer, done));
            }
        }
        let ends: Vec<(usize, bool)> = items.iter().map(|(at, _, done)| (*at, *done)).collect();
        drained.borrow_and_update();
        let told = replies.items(
            items
                .into_iter()
                .map(|(at, answer, done)| (&members[at].claim, members[at].id, answer, done)),
        );
        let gone: Vec<usize> = ends
            .into_iter()
            .zip(&told)
            .filter(|((_, done), told)| *done || !**told)
            .map(|((at, _), _)| at)
            .collect();
        let mut at = 0;
        members.retain(|_| {
            let kept = !gone.contains(&at);
            at += 1;
            kept
        });
        told.contains(&true)
    }
}
