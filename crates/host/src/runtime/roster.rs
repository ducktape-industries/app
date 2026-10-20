//! The networks the app holds, each a [`Network`]: its node [`Connection`], its roster and
//! its seats; the one in hand ([`in_hand`]); the rail rows drawn from a roster, the loads a
//! connect or a new block starts, and the reading of a `duck://` link against a roster.
use super::*;

/// The session facts every view is handed as its props: the signing key
/// unlocked in this session as `signer` (hex) and the `account` it holds,
/// `None` until one is resolved.
pub fn props(
    connected: bool,
    network: &str,
    key: &str,
    account: Option<u64>,
    endpoint: &str,
) -> Vec<u8> {
    wire::methods::encode(&wire::methods::Session {
        connected,
        chain_id: network.into(),
        signer: key.into(),
        account,
        endpoint: endpoint.into(),
    })
}

#[derive(Clone, Default)]
pub(super) struct Connection {
    pub(super) client: Option<crate::backend::RpcClient>,
    pub(super) network: String,
    pub(super) chain: String,
    pub(super) rev: u64,
    pub(super) live: Option<Arc<dyn crate::backend::live::Session>>,
    pub(super) on: On,
}

#[derive(Clone)]
pub(super) struct On(std::sync::Weak<Network>);

impl Default for On {
    fn default() -> Self {
        Self::of(in_hand())
    }
}

impl On {
    pub(super) fn of(network: &Arc<Network>) -> Self {
        Self(Arc::downgrade(network))
    }

    pub(super) fn get(&self) -> Option<Arc<Network>> {
        self.0.upgrade()
    }

    pub(super) fn followed(&self) -> bool {
        self.get().is_some_and(|network| network.followed)
    }

    pub(super) fn roster(&self) -> Roster {
        self.get()
            .map(|network| network.roster.clone())
            .unwrap_or_default()
    }
}

pub struct Network {
    pub(super) connection: Mutex<Connection>,
    pub(super) roster: Roster,
    pub(super) seats: Registry,
    pub(super) followed: bool,
    pub(super) left: std::sync::atomic::AtomicBool,
    reading: std::sync::atomic::AtomicBool,
}

impl Network {
    pub(super) fn new(followed: bool) -> Arc<Self> {
        let network = Arc::new(Self {
            connection: Mutex::new(Connection {
                client: None,
                network: String::new(),
                chain: String::new(),
                rev: 0,
                live: None,
                on: On(std::sync::Weak::new()),
            }),
            roster: Roster::default(),
            seats: Registry::default(),
            followed,
            left: Default::default(),
            reading: Default::default(),
        });
        lock(&network.connection).on = On::of(&network);
        network
    }

    fn take(&self, client: &crate::backend::RpcClient, network: &str, chain: &str) -> Connection {
        let mut connection = lock(&self.connection);
        connection.rev += 1;
        connection.client = Some(client.clone());
        connection.network = network.to_owned();
        connection.chain = chain.to_owned();
        connection.live = Some(Arc::new(crate::backend::live::Client::new(client.clone())));
        connection.clone()
    }

    pub fn connected(
        self: &Arc<Self>,
        client: &crate::backend::RpcClient,
        network: &str,
        chain: &str,
    ) {
        super::notifier::stop_every();
        let snapshot = self.take(client, network, chain);
        spawn_roster_read(self.clone(), snapshot, || {});
    }

    pub fn disconnected(&self) {
        let mut connection = lock(&self.connection);
        if connection.client.is_some() {
            *connection = Connection {
                rev: connection.rev + 1,
                on: connection.on.clone(),
                ..Connection::default()
            };
        }
    }

    pub fn deployments_checked(self: &Arc<Self>) {
        use std::sync::atomic::Ordering;
        if self.reading.swap(true, Ordering::SeqCst) {
            return;
        }
        let snapshot = lock(&self.connection).clone();
        let reading = self.clone();
        let done = move || reading.reading.store(false, Ordering::SeqCst);
        if snapshot.client.is_none() || !spawn_roster_read(self.clone(), snapshot, done) {
            self.reading.store(false, Ordering::SeqCst);
        }
    }
}

impl Connection {
    pub(super) fn stale(&self) -> bool {
        let Some(network) = self.on.get() else {
            return true;
        };
        match network.followed {
            true => network.left.load(std::sync::atomic::Ordering::SeqCst),
            false => lock(&network.connection).rev != self.rev,
        }
    }

    pub(super) fn current(&self) -> Connection {
        self.on
            .get()
            .map_or_else(|| self.clone(), |network| lock(&network.connection).clone())
    }
}

pub fn in_hand() -> &'static Arc<Network> {
    static IN_HAND: OnceLock<Arc<Network>> = OnceLock::new();
    IN_HAND.get_or_init(|| Network::new(false))
}

pub(super) fn connection() -> &'static Mutex<Connection> {
    &in_hand().connection
}

/// A roster as a node last listed it, every program and its code, and
/// which of them fill the roles the kernel calls; shared by whoever holds
/// a clone: the app's one ([`roster`]), which the node's reads fill, its
/// views load from and its windows draw, or a test's own. A change tells
/// the rail ([`changes_channel`]), never a window.
#[derive(Clone, Default)]
pub struct Roster {
    programs: Arc<Mutex<Vec<crate::backend::views::Program>>>,
    bound: Arc<Mutex<Bound>>,
}

#[derive(Default)]
struct Bound {
    asked_of: Option<u64>,
    roles: Option<abi::Roles>,
    due: Due,
}

#[derive(Clone, Copy, Default)]
enum Due {
    #[default]
    Unknown,
    Never,
    At(u64),
}

impl Roster {
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, Vec<crate::backend::views::Program>> {
        self.programs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn bound(&self) -> std::sync::MutexGuard<'_, Bound> {
        self.bound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The programs genesis bound to the roles the kernel calls (the
    /// programs role, the validators, identity), as the node's programs role
    /// answers them: `None` until read.
    pub fn roles(&self) -> Option<abi::Roles> {
        self.bound().roles.clone()
    }

    /// The role bindings as if the programs role had answered `roles`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn bind(&self, roles: Option<abi::Roles>) {
        self.bound().roles = roles;
    }

    /// A roster of its own that lists `programs`, for a test to draw or
    /// open.
    #[cfg(any(test, feature = "test-support"))]
    pub fn listing(programs: &[&str]) -> Self {
        let roster = Self::default();
        roster.lock().extend(
            programs
                .iter()
                .map(|program| crate::backend::views::Program {
                    name: (*program).into(),
                    code: abi::BlobId::Sha256([0; 32]),
                    bare: false,
                }),
        );
        roster
    }

    /// Lists what `listed` lists, as a node's read would: a test's.
    #[cfg(any(test, feature = "test-support"))]
    pub fn relist_from(&self, listed: &Roster) {
        let programs = listed.lock().clone();
        relist(self, programs);
    }

    pub(crate) fn clear(&self) {
        self.bound().due = Due::Unknown;
        relist(self, Vec::new());
    }

    pub(super) fn code(&self, program: &str) -> Option<(abi::BlobId, bool)> {
        self.lock()
            .iter()
            .find(|entry| entry.name == program)
            .map(|entry| (entry.code, entry.bare))
    }

    pub(super) fn runs(&self, code: &abi::BlobId) -> bool {
        self.lock().iter().any(|entry| entry.code == *code)
    }

    /// Whether it lists `program`: a link to anything else names nothing.
    pub fn lists(&self, program: &str) -> bool {
        self.code(program).is_some()
    }

    pub(super) fn due(&self, height: u64) -> bool {
        match self.bound().due {
            Due::Unknown => true,
            Due::Never => false,
            Due::At(at) => height.saturating_add(1) >= at,
        }
    }
}

/// The app's one roster.
pub fn roster() -> &'static Roster {
    &in_hand().roster
}

type Changes = futures::channel::mpsc::UnboundedSender<()>;

fn changes() -> &'static Mutex<Option<Changes>> {
    static CHANGES: OnceLock<Mutex<Option<Changes>>> = OnceLock::new();
    CHANGES.get_or_init(Mutex::default)
}

/// The rail's wake: one message each time the roster or a seat moved in a
/// way [`Roster::rail`] reads, from whichever thread moved it. Installing
/// it replaces the one before; the app installs one; of the tests only
/// `a_seat_claimed_or_dropped_wakes_the_rail` does (the others' rails are
/// refreshed by hand, and no loader thread wakes their scheduler).
pub fn changes_channel() -> futures::channel::mpsc::UnboundedReceiver<()> {
    let (send, receive) = futures::channel::mpsc::unbounded();
    *changes().lock().expect("roster changes") = Some(send);
    receive
}

pub(super) fn relist(
    roster: &Roster,
    programs: Vec<crate::backend::views::Program>,
) -> Vec<crate::backend::views::Program> {
    let mut listed = roster.lock();
    let previous = std::mem::replace(&mut *listed, programs);
    let moved = previous != *listed;
    drop(listed);
    if moved {
        rail_moved();
    }
    previous
}

pub(super) fn rail_moved() {
    if let Some(send) = &*changes().lock().expect("roster changes") {
        let _ = send.unbounded_send(());
    }
}

pub(super) fn code_digest(code: &abi::BlobId) -> [u8; 32] {
    let mut digest = [0; 32];
    let bytes = code.digest();
    digest[..bytes.len()].copy_from_slice(bytes);
    digest
}

/// The programs the rail lists, in roster order, with the name each one's
/// view gives itself (its program name until the manifest is read) and
/// whether it has a view at all.
#[derive(Clone, Debug, PartialEq)]
pub struct RailRow {
    pub program: &'static str,
    pub label: String,
    /// The icon its view's manifest declares (`icons/hammer.svg`); empty
    /// until the manifest is read, and for a view that declares none.
    pub icon: String,
    /// The tint its view's manifest declares: the colour of its tile on a
    /// phone's Home (the desktop rail draws none). `Tint::None` until the
    /// manifest is read, and for a view that declares none.
    pub tint: Tint,
    /// The manifest's description (Home's tile and product page); empty
    /// until the manifest is read.
    pub description: String,
    /// `Some` while the seat is on its way or failed; `None` once drawn.
    pub note: Option<&'static str>,
    /// The program ships no view: the rail leaves it out.
    pub empty: bool,
    /// Its code not read yet (no seat, the bytes still coming, or read and
    /// not shown until the rest of its roster read's are): whether it ships
    /// a view is not known, so `empty` is `false` meanwhile and the row says
    /// `Loading`. Home lists no program before this is known.
    pub pending: bool,
    /// How far its code has come on this device (its readiest seat's):
    /// what a count of the programs being prepared reads.
    pub prep: Prep,
}

/// How far a program's code has come on this device ([`RailRow::prep`]),
/// least far first. A roster read reads every program's bytes before any is
/// prepared ([`super::seat::read_ahead`]): `Reading` is that first half,
/// `Waiting` and `Working` the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prep {
    /// Its bytes not read yet (asked for, or coming).
    Reading,
    /// Its bytes here, no loader at its code yet.
    Waiting,
    /// A loader compiling its code.
    Working,
    /// Drawn, compiled, found to ship no view, or failed: nothing more is
    /// coming.
    Settled,
}

impl Prep {
    pub(super) fn of(stage: &Slot) -> Prep {
        match stage {
            Slot::Loading => Prep::Reading,
            Slot::Fetching { received, total } if *total != Some(*received) => Prep::Reading,
            Slot::Fetching { .. } | Slot::Listed(_) => Prep::Waiting,
            Slot::Compiling(_) => Prep::Working,
            _ => Prep::Settled,
        }
    }
}

fn readiness(slot: &Slot) -> u8 {
    match slot {
        Slot::Ready(view) if !view.name.is_empty() => 0,
        Slot::Compiled(view) if !view.name.is_empty() => 1,
        Slot::Ready(_) | Slot::Compiled(_) => 2,
        Slot::Empty => 3,
        Slot::Failed(_) => 4,
        Slot::Loading | Slot::Fetching { .. } | Slot::Listed(_) | Slot::Compiling(_) => 5,
    }
}

impl Roster {
    pub fn rail(&self) -> Vec<RailRow> {
        let _timed = crate::perf::time(crate::perf::Key::Shell, "rail");
        crate::perf::count(crate::perf::Key::Shell, "rail.calls", 1);
        let programs: Vec<&'static str> = self
            .lock()
            .iter()
            .map(|program| intern(&program.name))
            .collect();
        let registry = lock(registry());
        programs
            .into_iter()
            .map(|program| {
                let seat = registry
                    .iter()
                    .filter(|((name, _), _)| *name == program)
                    .min_by_key(|((_, instance), seat)| (readiness(&lock(seat).slot), *instance))
                    .map(|(_, seat)| lock(seat));
                let (label, note, empty) = match seat.as_ref().map(|seat| &seat.slot) {
                    Some(Slot::Ready(view)) if !view.name.is_empty() => {
                        (view.name.clone(), None, false)
                    }
                    Some(Slot::Compiled(view)) if !view.name.is_empty() => {
                        (view.name.clone(), None, false)
                    }
                    Some(Slot::Ready(_) | Slot::Compiled(_)) => (program.to_owned(), None, false),
                    Some(Slot::Listed(listing) | Slot::Compiling(listing))
                        if !listing.name.is_empty() =>
                    {
                        (listing.name.clone(), Some("Loading"), false)
                    }
                    Some(Slot::Empty) => (program.to_owned(), None, true),
                    Some(Slot::Failed(_)) => (program.to_owned(), Some("Failed"), false),
                    Some(_) | None => (program.to_owned(), Some("Loading"), false),
                };
                let pending = matches!(
                    seat.as_ref().map(|seat| &seat.slot),
                    None | Some(Slot::Loading | Slot::Fetching { .. })
                );
                let prep = seat.as_ref().map_or(Prep::Reading, |seat| seat.prep());
                let (icon, tint, description) = match seat.as_ref().map(|seat| &seat.slot) {
                    Some(Slot::Ready(view)) => {
                        (view.icon.clone(), view.tint, view.description.clone())
                    }
                    Some(Slot::Compiled(view)) => {
                        (view.icon.clone(), view.tint, view.description.clone())
                    }
                    Some(Slot::Listed(listing) | Slot::Compiling(listing)) => (
                        listing.icon.clone(),
                        listing.tint,
                        listing.description.clone(),
                    ),
                    _ => Default::default(),
                };
                RailRow {
                    program,
                    label,
                    icon,
                    tint,
                    description,
                    note,
                    empty,
                    pending,
                    prep,
                }
            })
            .collect()
    }

    /// The one reading of a link, against the views this roster lists.
    pub fn parse_link(&self, link: &str) -> Link {
        parse_link_among(link, &self.rail())
    }
}

/// A node was connected: every seat is asked again of it, and the roster
/// is read so the rail lists what the network runs.
pub fn connected(client: &crate::backend::RpcClient, network: &str, chain: &str) {
    in_hand().connected(client, network, chain);
}

/// The node in hand was let go (its network left): a read, a load or a
/// request still out to it is refused from now on (`rev` moves), so none
/// lands its answers after it, and the views are asked of no node until
/// the next connect. Nothing to let go when no node was connected.
pub fn disconnected() {
    in_hand().disconnected();
}

/// The roster is read again, and a
/// program whose code changed is loaded again, under a new generation, and
/// swapped in place when ready. One read in flight at a time; a block that
/// lands during one is covered by the next. The read alone holds that
/// latch: the loads it queues land on their own. Cheap when nothing moved.
pub fn deployments_checked() {
    in_hand().deployments_checked();
}

pub fn deployments_due(height: u64) -> bool {
    roster().due(height)
}

fn spawn_roster_read(
    network: Arc<Network>,
    asked_of: Connection,
    then: impl FnOnce() + Send + 'static,
) -> bool {
    let spawned = std::thread::Builder::new()
        .name("roster".into())
        .spawn(move || {
            let read = std::panic::AssertUnwindSafe(|| {
                if let Some(programs) =
                    read_roster(asked_of.clone(), &network.roster, &network.seats)
                {
                    super::notifier::reconcile_all(&asked_of, &programs);
                }
            });
            let _ = std::panic::catch_unwind(read);
            then();
        });
    if let Err(error) = &spawned {
        tracing::warn!(
            target: "ducktape::app",
            reason = "roster_unread",
            error = %error,
            "no thread to read the node's programs on"
        );
    }
    spawned.is_ok()
}

pub(super) fn read_roster(
    asked_of: Connection,
    roster: &Roster,
    registry: &Registry,
) -> Option<Vec<crate::backend::views::Program>> {
    let client = asked_of.client.as_ref()?;
    let read = crate::perf::time(crate::perf::Key::Shell, "roster");
    let programs = handle().block_on(crate::backend::views::programs(
        client,
        &asked_of.network,
        &*crate::backend::query_signer(),
    ));
    drop(read);
    let programs = match programs {
        Ok(programs) => programs,
        Err(error) => {
            tracing::warn!(
                target: "ducktape::app",
                reason = "roster_unreadable",
                error = %error,
                "the node's programs were not listed"
            );
            roster.bound().due = Due::Unknown;
            return None;
        }
    };
    let due = match handle().block_on(crate::backend::views::next_scheduled(
        client,
        &asked_of.network,
        &*crate::backend::query_signer(),
    )) {
        Ok(Some(height)) => Due::At(height),
        Ok(None) => Due::Never,
        Err(error) => {
            tracing::warn!(
                target: "ducktape::app",
                reason = "schedule_unreadable",
                error = %error,
                "the node's scheduled changes were not read"
            );
            Due::Unknown
        }
    };
    let roles_due = roster.bound().asked_of != Some(asked_of.rev) || *roster.lock() != programs;
    let roles = if roles_due {
        match handle().block_on(crate::backend::views::roles(
            client,
            &asked_of.network,
            &*crate::backend::query_signer(),
        )) {
            Ok(roles) => Some(roles),
            Err(error) => {
                tracing::warn!(
                    target: "ducktape::app",
                    reason = "roles_unreadable",
                    error = %error,
                    "the node's role bindings were not read"
                );
                roster.bound().due = Due::Unknown;
                return None;
            }
        }
    } else {
        None
    };
    let loads = {
        let node_since_left = asked_of.stale();
        if node_since_left {
            return None;
        }
        let mut bound = roster.bound();
        if let Some(roles) = roles {
            bound.asked_of = Some(asked_of.rev);
            bound.roles = Some(roles);
        }
        bound.due = due;
        drop(bound);
        if crate::contract::os().limits().preload == crate::contract::Preload::Headless {
            relist(roster, programs.clone());
            return Some(programs);
        }
        let mut registry = lock(registry);
        let names: Vec<&'static str> = programs.iter().map(|p| intern(&p.name)).collect();
        for gone in registry
            .keys()
            .copied()
            .filter(|(program, _)| !names.contains(program))
            .collect::<Vec<_>>()
        {
            if let Some(retired) = registry.get(&gone) {
                lock(retired).retire();
            }
            if gone.1 == 0 {
                registry.remove(&gone);
            }
        }
        let previous = {
            let mut listed = roster.lock();
            if asked_of.stale() {
                return None;
            }
            std::mem::replace(&mut *listed, programs.clone())
        };
        if previous != programs {
            rail_moved();
        }
        let mut loads = Vec::new();
        for program in &names {
            if !registry.keys().any(|(name, _)| name == program) {
                registry.insert((*program, 0), Mounted::seat());
            }
        }
        for ((program, _), seat) in registry.iter() {
            let Some(listed) = programs.iter().find(|listed| listed.name == *program) else {
                continue;
            };
            let mut locked = lock(seat);
            let same_code = previous
                .iter()
                .any(|old| old.name == listed.name && old.code == listed.code);
            let asked_of_this_node = locked.generation > 0 && locked.rev == asked_of.rev;
            let code = code_digest(&listed.code);
            if !locked.reload_due(same_code, asked_of_this_node, code) {
                continue;
            }
            locked.rev = asked_of.rev;
            let generation = locked.start();
            if !same_code && !locked.slot.loading() {
                locked.replacing = Some((generation, Prep::Reading));
            }
            loads.push(Load {
                program,
                seat: seat.clone(),
                generation,
                asked_of: asked_of.clone(),
                code: Some((listed.code, listed.bare)),
            });
        }
        loads.sort_by_key(|load| names.iter().position(|name| *name == load.program));
        loads
    };
    let (claimed, preloads): (Vec<Load>, Vec<Load>) = loads
        .into_iter()
        .partition(|load| lock(&load.seat).instance != 0);
    for load in claimed {
        queue(load);
    }
    super::seat::read_ahead(preloads);
    Some(programs)
}

/// What a link names, as far as the app can tell without the chain in
/// hand. The `duck://` grammar is shared with the programs repo: this reads it,
/// it does not change it.
#[derive(Debug, PartialEq)]
pub enum Link {
    /// A view on this connection by its short name (`duck://<view>`, or
    /// `duck://<view>/<route>` with the route its view reads).
    View {
        program: &'static str,
        route: Option<String>,
    },
    /// `duck://<chain>/<program>/<tail>`: which chain is for the caller.
    Chain(ducklink::Link),
    /// An `http(s)` page, for the system browser.
    Web(String),
    /// Nothing this app opens.
    Unknown,
}

fn parse_link_among(link: &str, rows: &[RailRow]) -> Link {
    if let Some(program) = local_seat(link, rows) {
        return Link::View {
            program,
            route: None,
        };
    }
    match ducklink::Link::parse(link) {
        Ok(parsed) => Link::Chain(parsed),
        Err(_) if link.starts_with("http://") || link.starts_with("https://") => {
            Link::Web(link.to_owned())
        }
        Err(_) => match local_seat_route(link, rows) {
            Some((program, route)) => Link::View {
                program,
                route: Some(route),
            },
            None => Link::Unknown,
        },
    }
}

fn local_seat_route(link: &str, rows: &[RailRow]) -> Option<(&'static str, String)> {
    let (seat, route) = link.strip_prefix("duck://")?.split_once('/')?;
    let program = local_seat(&format!("duck://{seat}"), rows)?;
    let route = ducklink::tail(route).ok()?.join("/");
    valid_route(&route).then_some((program, route))
}

/// A DECODED route a link may hand a view (`host.route`): at most 256 bytes
/// of `/`-separated segments, each nonempty, not `.` or `..`, and free of
/// control characters. The link spelled it percent-encoded; `ducklink`
/// decoded it.
pub fn valid_route(route: &str) -> bool {
    !route.is_empty()
        && route.len() <= 256
        && !route.contains(char::is_control)
        && route
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn local_seat(link: &str, rows: &[RailRow]) -> Option<&'static str> {
    let name = link.strip_prefix("duck://")?;
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return None;
    }
    if let Some(row) = rows.iter().find(|row| row.program == name && !row.empty) {
        return Some(row.program);
    }
    let mut matches = rows
        .iter()
        .filter(|row| !row.empty && row.note.is_none() && row.label.to_lowercase() == name);
    let first = matches.next()?;
    matches.next().is_none().then_some(first.program)
}

#[cfg(test)]
mod local_link_tests {
    use super::*;

    #[test]
    fn short_links_resolve_ids_and_unique_ready_labels() {
        let mut rows = vec![RailRow {
            program: "catalog",
            label: "Preferences".into(),
            icon: String::new(),
            tint: Tint::None,
            description: String::new(),
            note: None,
            empty: false,
            pending: false,
            prep: Prep::Settled,
        }];
        assert_eq!(local_seat("duck://catalog", &rows), Some("catalog"));
        assert_eq!(local_seat("duck://preferences", &rows), Some("catalog"));
        for link in [
            "duck://",
            "duck://Preferences",
            "duck://network-abcd1234/preferences",
            "duck://preferences?x",
            "https://preferences",
        ] {
            assert_eq!(local_seat(link, &rows), None);
        }
        assert_eq!(
            local_seat_route("duck://catalog/tx/00ff", &rows),
            Some(("catalog", "tx/00ff".into()))
        );
        assert_eq!(
            local_seat_route("duck://preferences/a.b_c-D", &rows),
            Some(("catalog", "a.b_c-D".into()))
        );
        for link in [
            "duck://catalog/",
            "duck://catalog/tx//00ff",
            "duck://catalog/../x",
            "duck://catalog/tx?x",
            "duck://unknown/tx/00ff",
        ] {
            assert_eq!(local_seat_route(link, &rows), None, "{link}");
        }
        assert!(valid_route(&"a".repeat(256)));
        assert!(!valid_route(&"a".repeat(257)));
        rows.push(RailRow {
            program: "other",
            label: "Preferences".into(),
            icon: String::new(),
            tint: Tint::None,
            description: String::new(),
            note: None,
            empty: false,
            pending: false,
            prep: Prep::Settled,
        });
        assert_eq!(local_seat("duck://preferences", &rows), None);
        rows[1].empty = true;
        assert_eq!(local_seat("duck://preferences", &rows), Some("catalog"));
        rows[0].note = Some("Loading");
        assert_eq!(local_seat("duck://preferences", &rows), None);
    }

    #[test]
    fn a_link_reads_as_one_thing() {
        let rows = [RailRow {
            program: "catalog",
            label: "Preferences".into(),
            icon: String::new(),
            tint: Tint::None,
            description: String::new(),
            note: None,
            empty: false,
            pending: false,
            prep: Prep::Settled,
        }];
        let view = |route: Option<&str>| Link::View {
            program: "catalog",
            route: route.map(str::to_owned),
        };
        assert_eq!(parse_link_among("duck://catalog", &rows), view(None));
        assert_eq!(parse_link_among("duck://preferences", &rows), view(None));
        assert_eq!(
            parse_link_among("duck://catalog/tx/00ff", &rows),
            view(Some("tx/00ff"))
        );
        assert!(matches!(
            parse_link_among("duck://testkit-0a1b2c3d/chat/tx/00ff", &rows),
            Link::Chain(link) if link.program == "chat"
        ));
        assert_eq!(
            parse_link_among("https://ducktape.dev/x", &rows),
            Link::Web("https://ducktape.dev/x".into())
        );
        for link in ["ftp://x", "duck://unknown/../x", "catalog"] {
            assert_eq!(parse_link_among(link, &rows), Link::Unknown, "{link}");
        }
    }

    #[test]
    fn a_route_is_delivered_decoded() {
        let rows = [RailRow {
            program: "catalog",
            label: "Preferences".into(),
            icon: String::new(),
            tint: Tint::None,
            description: String::new(),
            note: None,
            empty: false,
            pending: false,
            prep: Prep::Settled,
        }];
        for name in ["forge:web:3", "보고서 #1", "a b"] {
            let link = ducklink::mint("testkit#0a1b2c3d", "chat", &[name, "42"]).unwrap();
            let Link::Chain(parsed) = parse_link_among(&link, &rows) else {
                panic!("{link}");
            };
            let route = parsed.tail.join("/");
            assert_eq!(route, format!("{name}/42"));
            assert!(valid_route(&route), "{route}");
            let short = link.replacen("testkit-0a1b2c3d/chat", "catalog", 1);
            assert_eq!(
                parse_link_among(&short, &rows),
                Link::View {
                    program: "catalog",
                    route: Some(route)
                },
                "{short}"
            );
        }
        assert!(link_to_catalog("duck://catalog/forge%3Aweb%3A3", &rows));
        for broken in [
            "duck://catalog/forge%3aweb",
            "duck://catalog/forge%3",
            "duck://catalog/%ZZ",
            "duck://catalog/a%2Fb",
            "duck://catalog/a%0Ab",
            "duck://catalog/forge:web",
        ] {
            assert!(!link_to_catalog(broken, &rows), "{broken}");
        }
        assert!(!valid_route("a\u{7f}b"), "a control character");
        assert!(!valid_route(&"보".repeat(86)), "258 decoded bytes");
    }

    fn link_to_catalog(link: &str, rows: &[RailRow]) -> bool {
        matches!(
            parse_link_among(link, rows),
            Link::View { route: Some(_), .. }
        )
    }
}

#[cfg(test)]
mod rail_tests {
    use super::*;

    #[test]
    fn unmounted_program_is_loading_not_a_bare_id() {
        let program = "rail-tests-unmounted";
        let row = Roster::listing(&[program])
            .rail()
            .into_iter()
            .find(|row| row.program == program)
            .expect("listed program appears in the rail");
        assert_eq!(row.label, program);
        assert_eq!(row.note, Some("Loading"));
        assert!(!row.empty);
        assert!(row.pending, "whether it ships a view is not known yet");
        assert_eq!(row.prep, Prep::Reading, "no seat yet: its bytes not read");
    }

    #[test]
    fn a_compiled_seat_is_listed_by_its_manifest_name() {
        let program = "rail-tests-compiled";
        let seat = Mounted::seat();
        lock(&seat).slot = Slot::Compiled(ViewCode::for_test("Compiled view", "", 320));
        lock(registry()).insert((program, 0), seat);
        let row = Roster::listing(&[program])
            .rail()
            .into_iter()
            .find(|row| row.program == program)
            .expect("listed program appears in the rail");
        lock(registry()).remove(&(program, 0));
        assert_eq!((row.label.as_str(), row.note), ("Compiled view", None));
        assert!(!row.empty && !row.pending);
    }

    #[test]
    fn a_program_is_pending_until_its_code_is_read() {
        let program = "rail-tests-pending";
        let seat = Mounted::seat();
        let row = |seat: &Arc<Mutex<Mounted>>, slot| {
            lock(seat).slot = slot;
            lock(registry()).insert((program, 0), seat.clone());
            let row = Roster::listing(&[program]).rail().remove(0);
            lock(registry()).remove(&(program, 0));
            (row.empty, row.pending, row.prep)
        };
        let fetching = Slot::Fetching {
            received: 1,
            total: Some(2),
        };
        assert_eq!(row(&seat, fetching), (false, true, Prep::Reading));
        let read = Slot::Fetching {
            received: 2,
            total: Some(2),
        };
        assert_eq!(row(&seat, read), (false, true, Prep::Waiting));
        assert_eq!(row(&seat, Slot::Empty), (true, false, Prep::Settled));
    }

    #[test]
    fn a_listed_seat_is_named_by_its_manifest_and_still_loading() {
        let program = "rail-tests-listed";
        let listing = || {
            Box::new(Listing {
                name: "Listed view".into(),
                icon: "icons/hammer.svg".into(),
                tint: Tint::Orange,
                description: "What it does".into(),
                ..Default::default()
            })
        };
        let seat = Mounted::seat();
        lock(registry()).insert((program, 0), seat.clone());
        let row = |slot: Slot| {
            lock(&seat).slot = slot;
            Roster::listing(&[program])
                .rail()
                .into_iter()
                .find(|row| row.program == program)
                .expect("listed program appears in the rail")
        };
        let rows = [
            row(Slot::Listed(listing())),
            row(Slot::Compiling(listing())),
        ];
        lock(registry()).remove(&(program, 0));
        for row in rows {
            assert_eq!(
                (
                    row.label.as_str(),
                    row.icon.as_str(),
                    row.tint,
                    row.description.as_str()
                ),
                (
                    "Listed view",
                    "icons/hammer.svg",
                    Tint::Orange,
                    "What it does"
                )
            );
            assert_eq!(row.note, Some("Loading"));
        }
    }

    #[test]
    fn a_program_with_two_seats_is_named_by_its_readiest() {
        let program = "rail-tests-two-seats";
        let named = Mounted::seat();
        lock(&named).slot = Slot::Compiled(ViewCode::for_test("Nodes", "", 320));
        let instances = 1..=16u64;
        {
            let mut registry = lock(registry());
            for instance in instances.clone() {
                registry.insert((program, instance), Mounted::seat());
            }
            registry.insert((program, 9), named);
        }
        let roster = Roster::listing(&[program]);
        let rows: Vec<RailRow> = (0..8)
            .flat_map(|_| roster.rail())
            .filter(|row| row.program == program)
            .collect();
        {
            let mut registry = lock(registry());
            for instance in instances {
                registry.remove(&(program, instance));
            }
        }
        assert_eq!(rows.len(), 8);
        for row in rows {
            assert_eq!((row.label.as_str(), row.note), ("Nodes", None));
        }
    }

    #[test]
    fn a_view_only_entry_is_a_rail_row_and_a_short_link() {
        let view = abi::BlobId::Sha256([7; 32]);
        let roster = Roster::default();
        roster.lock().push(crate::backend::views::Program {
            name: "explorer".into(),
            code: view,
            bare: true,
        });
        assert!(roster.rail().iter().any(|row| row.program == "explorer"));
        assert_eq!(
            roster.parse_link("duck://explorer"),
            Link::View {
                program: "explorer",
                route: None
            }
        );
        assert_eq!(roster.code("explorer"), Some((view, true)));
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;

    #[tokio::test]
    async fn leaving_one_network_stales_only_its_own_work() {
        let (a, b) = (Network::new(false), Network::new(false));
        let client = crate::backend::rpc_client("http://127.0.0.1:9");
        let on_a = a.take(&client, "a", "a#1");
        let on_b = b.take(&client, "b", "b#1");
        assert!(!on_a.stale() && !on_b.stale());
        a.disconnected();
        assert!(on_a.stale(), "a's own work is refused once it left");
        assert!(!on_b.stale(), "b's is not");
        assert_eq!(on_b.current().chain, "b#1");
        drop(b);
        assert!(on_b.stale(), "a network let go stales its work");
    }

    #[test]
    fn each_network_holds_its_own_seats() {
        let program = "network-tests-seats";
        let network = Network::new(false);
        let seat = crate::runtime::seat::mounted(&network, program, 7);
        assert!(lock(&network.seats).contains_key(&(program, 7)));
        assert!(
            !lock(registry()).keys().any(|(name, _)| *name == program),
            "the network in hand holds no seat of it"
        );
        drop(seat);
        crate::runtime::seat::let_go(&network, program, 7);
        assert!(lock(&network.seats).is_empty());
    }
}
