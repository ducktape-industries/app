//! The other listed networks' notifiers, run on the desktop beside the
//! network in hand's: a FOLLOWED set per chain ([`follow`]), the person's
//! recipient on a connection of its own ([`Apart`]: its own end, its own
//! roster, its queries signed by the reader), polled as the network in hand
//! is, never touched by a connect. One run per chain at a time: taking a
//! chain up in hand ([`take_up`]) stops its set and hands its log to the
//! app's centre in memory, and the chain left parks its log for its own
//! set. The registry of chains here is that rule's one place, and it holds
//! the app's lock too ([`lock_followed`]), so a set starts as the app is.
use super::*;
use notify::Log;
use roster::Apart;

/// How often a followed set's node is asked its status: as the network in
/// hand's is, no faster.
pub const STATUS_EVERY: Duration = Duration::from_secs(2);

/// The chain id `duck://` links name of the node answering `status`: its
/// network and genesis salt; the name alone when the genesis yields none.
pub fn chain_of(status: &crate::backend::NodeStatus) -> String {
    ducklink::ChainId::of(&status.network, &status.genesis)
        .map_or_else(|| status.network.clone(), |chain| chain.to_string())
}

#[derive(Default)]
struct Sets {
    in_hand: (String, Option<CenterHandle>),
    running: HashMap<String, std::sync::Weak<Set>>,
    parked: HashMap<PathBuf, Log>,
}

fn sets() -> std::sync::MutexGuard<'static, Sets> {
    static SETS: OnceLock<Mutex<Sets>> = OnceLock::new();
    lock(SETS.get_or_init(Mutex::default))
}

static LOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app locked, or unlocked (the desktop's follower carries each move;
/// it starts unlocked): every followed set's notifiers run on, with no key
/// and no account while it is locked, and a set followed from now on
/// starts as it is.
pub fn lock_followed(locked: bool) {
    LOCKED.store(locked, std::sync::atomic::Ordering::SeqCst);
    let followed = sets().followed();
    for set in followed {
        set.props_moved();
    }
}

impl Sets {
    fn taken(&self, chain: &str) -> bool {
        self.in_hand.0 == chain
            || self
                .running
                .get(chain)
                .is_some_and(|set| set.strong_count() > 0)
    }

    fn followed(&self) -> Vec<Arc<Set>> {
        self.running
            .values()
            .filter_map(std::sync::Weak::upgrade)
            .collect()
    }

    fn park(&mut self, log: Log) {
        if let Some(file) = log.file() {
            self.parked.insert(file.to_owned(), log);
        }
    }

    fn unpark(&mut self, chain: &str) -> Option<Log> {
        notify::log_file(chain).and_then(|file| self.parked.remove(&file))
    }
}

/// `chain` taken up in hand by `center` (the app's; `""`: none, the Connect
/// screen), as a connect lands: the set following it, if one runs, leaves
/// ([`Set::leave`]) and hands its log over, else its parked log comes, else
/// its file's; the chain left behind parks its log for its own set
/// ([`follow`]). One run per chain at any moment, and a parked log taken up
/// over its file's. Another node of the chain in hand moves nothing.
pub fn take_up(center: &CenterHandle, chain: &str) {
    let mut sets = sets();
    sets.in_hand = (chain.to_owned(), Some(center.clone()));
    if center.lock().network() == chain {
        return;
    }
    let log = match sets.running.remove(chain).and_then(|set| set.upgrade()) {
        Some(set) => set.leave(),
        None => sets.unpark(chain).unwrap_or_else(|| Log::of(chain)),
    };
    let left = center.lock().put_log(log);
    sets.park(left);
}

pub(in crate::runtime) fn holding(chain: &str) -> Option<CenterHandle> {
    let sets = sets();
    if sets.in_hand.0 == chain {
        return sets.in_hand.1.clone();
    }
    let set = sets.running.get(chain)?.upgrade()?;
    Some(set.recipient().center.clone())
}

/// The node the set following `chain` asks: where a click on one of its
/// notices switches to. `None` when no set follows it.
pub fn followed_url(chain: &str) -> Option<String> {
    let set = sets().running.get(chain)?.upgrade()?;
    Some(set.url.clone())
}

pub(super) fn permission_moved(program: &str, permission: Permission) {
    let sets = sets().followed();
    for set in sets {
        match permission {
            Permission::Block => set.notifiers.stop(program),
            _ => set.reconcile(),
        }
    }
}

pub(in crate::runtime) fn flush_sets() {
    let followed = sets().followed();
    for set in followed {
        set.recipient().center.lock().flush();
    }
}

struct Set {
    notifiers: Notifiers,
    connection: Mutex<Connection>,
    apart: Arc<Apart>,
    url: String,
    key: String,
    account: Mutex<Option<u64>>,
    poll: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Set {
    fn recipient(&self) -> &Recipient {
        self.notifiers
            .recipient
            .as_deref()
            .expect("a followed set has a recipient")
    }

    fn props_moved(&self) {
        self.recipient().session.send_if_modified(|props| {
            let account = *lock(&self.account);
            let (key, account) = match LOCKED.load(std::sync::atomic::Ordering::SeqCst) {
                true => ("", None),
                false => (&self.key[..], account),
            };
            let chain = lock(&self.connection).chain.clone();
            let session = roster::props(true, &chain, key, account, &self.url);
            let moved = props.as_ref() != Some(&session);
            *props = Some(session);
            moved
        });
    }

    fn reconcile(&self) {
        let _center = self.recipient().center.lock();
        if self.apart.left.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let asked_of = lock(&self.connection).clone();
        let programs = self.apart.roster.lock().clone();
        self.notifiers.reconcile(&asked_of, &programs);
    }

    async fn read(&self) {
        let (client, network) = {
            let connection = lock(&self.connection);
            (connection.client.clone(), connection.network.clone())
        };
        let Some(client) = client else {
            return;
        };
        let reader = crate::contract::os().reader();
        match crate::backend::views::programs(&client, &network, &*reader).await {
            Ok(programs) => *self.apart.roster.lock() = programs,
            Err(error) => {
                tracing::debug!(target: "ducktape::app", %error, network, "a followed network's programs were not listed");
                return;
            }
        }
        self.reconcile();
        let unknown = lock(&self.account).is_none();
        if unknown && let Some(account) = account_of(&client, &network, &self.key).await {
            *lock(&self.account) = Some(account);
            self.props_moved();
        }
    }

    fn leave(&self) -> Log {
        let mut center = self.recipient().center.lock();
        self.apart
            .left
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.notifiers.stop_all();
        if let Some(poll) = lock(&self.poll).take() {
            poll.abort();
        }
        center.take_log()
    }
}

/// A followed set, running while this lives ([`follow`]): dropped (its
/// network forgotten, or now in hand), it leaves, and its log parks for its
/// chain's next run.
pub struct Followed(Arc<Set>);

impl Followed {
    /// The chain it follows.
    pub fn chain(&self) -> String {
        lock(&self.0.connection).chain.clone()
    }

    /// The key and account its notifiers are handed now.
    #[cfg(any(test, feature = "test-support"))]
    pub fn handed(&self) -> (String, Option<u64>) {
        let props = self.0.recipient().session.borrow().clone();
        let session: wire::methods::Session =
            wire::methods::decode(&props.unwrap_or_default()).expect("its session");
        (session.signer, session.account)
    }

    /// The centre holding its chain's log, for a test to log a row into.
    #[cfg(any(test, feature = "test-support"))]
    pub fn center(&self) -> CenterHandle {
        self.0.recipient().center.clone()
    }
}

impl Drop for Followed {
    fn drop(&mut self) {
        let mut sets = sets();
        let chain = self.chain();
        let ours = sets
            .running
            .get(&chain)
            .is_some_and(|set| std::ptr::eq(set.as_ptr(), Arc::as_ptr(&self.0)));
        if ours {
            sets.running.remove(&chain);
        }
        let log = self.0.leave();
        sets.park(log);
    }
}

/// Another listed network's notifiers, followed here beside the network in
/// hand's. The node at `url` is asked its status (as the phone's registrar
/// asks each listed node, `crates/phone/src/push.rs`), this device's key
/// there found by its public half alone (no key store asked: no Touch ID,
/// no password) and its account by the reader; then a set on a connection
/// of its own ([`Apart`]) runs its notifiers on its roster, read again at
/// each of its blocks ([`STATUS_EVERY`]), into the person's store and its
/// chain's own log, its banners the OS's, its badges dropped. `None` while
/// its node does not answer within [`STATUS_EVERY`], for the chain in
/// hand, and for one a set follows already. Resolves once its first read's
/// loads are done, so sets started one after another never load at once.
/// It never blocks: its files are read on the blocking pool.
pub async fn follow(url: String) -> Option<Followed> {
    let client = crate::backend::rpc_client(url.clone());
    let status = tokio::time::timeout(STATUS_EVERY, client.status())
        .await
        .ok()?
        .ok()?;
    if status.contract != noded_client::wire::NODE_CONTRACT {
        return None;
    }
    let chain = chain_of(&status);
    if sets().taken(&chain) {
        return None;
    }
    let remotes = crate::contract::os().dirs().remotes.clone();
    let (network, founded) = (status.network.clone(), status.time);
    let (of, file) = (chain.clone(), notify::log_file(&chain));
    let (key, read) = tokio::task::spawn_blocking(move || {
        let key = noded_client::keyring::bind(&remotes, &network, founded)
            .ok()
            .and_then(|keyring| crate::backend::device_key_of(&keyring.dir));
        (key.unwrap_or_default(), Log::read(&of, file))
    })
    .await
    .ok()?;
    let account = account_of(&client, &status.network, &key).await;
    let set = Set::new(
        url,
        chain,
        status.network.clone(),
        Some(client),
        key,
        account,
    );
    let followed = register(set.clone(), read)?;
    set.read().await;
    let _ = loads().subscribe().wait_for(|loads| *loads == 0).await;
    let heard = Heard {
        height: status.height,
        lost: false,
    };
    *lock(&set.poll) = Some(handle().spawn(poll(Arc::downgrade(&set), heard)));
    Some(followed)
}

/// A set following `chain` at `url` that runs no notifier and asks no node:
/// one a test switches to, or clicks a notice of, for this device's `key`
/// there and the `account` it holds.
#[cfg(any(test, feature = "test-support"))]
pub fn follow_for_test(
    url: &str,
    chain: &str,
    key: &str,
    account: Option<u64>,
) -> Option<Followed> {
    let set = Set::new(
        url.into(),
        chain.into(),
        String::new(),
        None,
        key.into(),
        account,
    );
    register(set, Log::of(chain))
}

impl Set {
    fn new(
        url: String,
        chain: String,
        network: String,
        client: Option<crate::backend::RpcClient>,
        key: String,
        account: Option<u64>,
    ) -> Arc<Self> {
        let apart = Arc::new(Apart::default());
        let recipient = Recipient {
            session: tokio::sync::watch::channel(None).0,
            store: super::store::store_of(None),
            center: CenterHandle::default(),
            settings: None,
            banners: Arc::new(OsBanners),
            said: futures::channel::mpsc::unbounded().0,
        };
        Arc::new(Set {
            notifiers: Notifiers {
                running: Mutex::default(),
                recipient: Some(Arc::new(recipient)),
            },
            connection: Mutex::new(Connection {
                client,
                network,
                chain,
                rev: 0,
                live: None,
                apart: Some(apart.clone()),
            }),
            apart,
            url,
            key,
            account: Mutex::new(account),
            poll: Mutex::default(),
        })
    }

    fn polled(&self, heard: &mut Heard, answer: Option<u64>) -> bool {
        let Some(height) = answer else {
            heard.lost = true;
            return false;
        };
        let back = std::mem::take(&mut heard.lost);
        if back {
            lock(&self.connection).rev += 1;
        }
        std::mem::replace(&mut heard.height, height) != height || back
    }
}

struct Heard {
    height: u64,
    lost: bool,
}

fn register(set: Arc<Set>, read: Log) -> Option<Followed> {
    let chain = lock(&set.connection).chain.clone();
    let mut sets = sets();
    if sets.taken(&chain) {
        return None;
    }
    let log = sets.unpark(&chain).unwrap_or(read);
    set.recipient().center.lock().put_log(log);
    sets.running.insert(chain, Arc::downgrade(&set));
    drop(sets);
    set.props_moved();
    Some(Followed(set))
}

async fn account_of(client: &crate::backend::RpcClient, network: &str, key: &str) -> Option<u64> {
    let key = noded_client::hex::hex_decode(key)
        .ok()
        .filter(|key| !key.is_empty())?;
    let reader = crate::contract::os().reader();
    let account = noded_client::identity::account_of_key(client, network, key, &*reader).await;
    Some(account.ok()??.0)
}

async fn poll(set: std::sync::Weak<Set>, mut heard: Heard) {
    loop {
        tokio::time::sleep(STATUS_EVERY).await;
        let Some(set) = set.upgrade() else {
            return;
        };
        let Some(client) = lock(&set.connection).client.clone() else {
            return;
        };
        let answer = tokio::time::timeout(STATUS_EVERY, client.status()).await;
        let height = answer.ok().and_then(Result::ok).map(|status| status.height);
        if set.polled(&mut heard, height) {
            set.read().await;
        }
    }
}

struct OsBanners;

impl Banners for OsBanners {
    fn post(&self, notice: &notify::Notice, click: Option<notify::Click>) -> bool {
        crate::contract::os().banners().post(notice, click)
    }

    fn withdraw(&self, chain: &str, tag: &str) {
        crate::contract::os().banners().withdraw(chain, tag)
    }
}

#[cfg(test)]
mod tests;
