//! The node in hand and what it runs: the global [`Connection`], the roster
//! (the node's program list and each one's code; the app keeps one, reached
//! through [`roster`], and a test builds its own), the rail rows the shell
//! draws from it, the loads a connect or a new block starts, and the reading
//! of a `duck://` link against the views this connection lists.
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

/// The node the views are asked of, and the network its frames name;
/// `rev` moves on every (re)connect so a load or a request from before it
/// is refused rather than landed on the wrong node.
#[derive(Clone, Default)]
pub(super) struct Connection {
    pub(super) client: Option<crate::backend::RpcClient>,
    pub(super) network: String,
    /// The chain id (`<network>#<salt>`) a view's `store` is kept under.
    pub(super) chain: String,
    pub(super) rev: u64,
}

pub(super) fn connection() -> &'static Mutex<Connection> {
    static CONNECTION: OnceLock<Mutex<Connection>> = OnceLock::new();
    CONNECTION.get_or_init(Mutex::default)
}

/// A roster as a node last listed it, every program and its code, and
/// which of them fill the roles the kernel calls; shared by whoever holds
/// a clone: the app's one ([`roster`]), which the node's reads fill, its
/// views load from and its windows draw, or a test's own. A change tells
/// the rail ([`changes_channel`]), never a window.
#[derive(Clone, Default)]
pub(crate) struct Roster {
    programs: Arc<Mutex<Vec<crate::backend::views::Program>>>,
    bound: Arc<Mutex<Bound>>,
}

/// The roles as the registry answered them, and the connection (`rev`)
/// that was asked: they are asked once per connection, and again when the
/// roster moved (a registry upgrade is a move), never each block.
#[derive(Default)]
struct Bound {
    asked_of: Option<u64>,
    roles: Option<abi::Roles>,
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
    /// registry, the validators, identity), as the node's registry answers
    /// them: `None` until read, and from a registry that refuses the
    /// question (one from before it).
    pub(crate) fn roles(&self) -> Option<abi::Roles> {
        self.bound().roles.clone()
    }

    /// A roster of its own that lists `modules`, for a test to draw or
    /// open.
    #[cfg(test)]
    pub(crate) fn listing(modules: &[&str]) -> Self {
        let roster = Self::default();
        roster
            .lock()
            .extend(modules.iter().map(|module| crate::backend::views::Program {
                name: (*module).into(),
                code: abi::BlobId::Sha256([0; 32]),
                bare: false,
            }));
        roster
    }

    /// The blob a listed module's view comes from, and whether that blob is
    /// the view itself (a view-only entry) rather than a program carrying it.
    pub(super) fn code(&self, module: &str) -> Option<(abi::BlobId, bool)> {
        self.lock()
            .iter()
            .find(|program| program.name == module)
            .map(|program| (program.code, program.bare))
    }

    /// Whether it lists `module`: a link to anything else names nothing.
    pub(crate) fn lists(&self, module: &str) -> bool {
        self.code(module).is_some()
    }
}

/// The app's one roster.
pub(crate) fn roster() -> &'static Roster {
    static ROSTER: OnceLock<Roster> = OnceLock::new();
    ROSTER.get_or_init(Roster::default)
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
pub(crate) fn changes_channel() -> futures::channel::mpsc::UnboundedReceiver<()> {
    let (send, receive) = futures::channel::mpsc::unbounded();
    *changes().lock().expect("roster changes") = Some(send);
    receive
}

/// `programs` in place of what `roster` listed, the one before handed
/// back. A list that moved wakes the rail: a program that left starts no
/// load, so this is the only word the rail gets of it.
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

/// The rows [`Roster::rail`] reads may have moved: the rail is told.
pub(super) fn rail_moved() {
    if let Some(send) = &*changes().lock().expect("roster changes") {
        let _ = send.unbounded_send(());
    }
}

/// A code id as the 32-byte hash a seat records: sha256 as is, sha1 padded.
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
    pub module: &'static str,
    pub label: String,
    /// `Some` while the seat is on its way or failed; `None` once drawn.
    pub note: Option<&'static str>,
    /// The program ships no view: the rail leaves it out.
    pub empty: bool,
}

impl Roster {
    pub(crate) fn rail(&self) -> Vec<RailRow> {
        let _timed = crate::perf::time(crate::perf::Key::Shell, "rail");
        crate::perf::count(crate::perf::Key::Shell, "rail.calls", 1);
        // the names first, and the roster let go before the seats are
        // locked: a roster read locks the seats, then the roster
        let programs: Vec<&'static str> = self
            .lock()
            .iter()
            .map(|program| intern(&program.name))
            .collect();
        let registry = lock(registry());
        programs
            .into_iter()
            .map(|module| {
                let seat = registry
                    .iter()
                    .find_map(|((name, _), seat)| (*name == module).then_some(seat))
                    .map(|seat| lock(seat));
                let (label, note, empty) = match seat.as_ref().map(|seat| &seat.slot) {
                    Some(Slot::Ready(guest)) if !guest.name.is_empty() => {
                        (guest.name.clone(), None, false)
                    }
                    // compiled, not started: as ready to open as a drawn view
                    Some(Slot::Compiled { name, .. }) if !name.is_empty() => {
                        (name.clone(), None, false)
                    }
                    Some(Slot::Ready(_) | Slot::Compiled { .. }) => {
                        (module.to_owned(), None, false)
                    }
                    Some(Slot::Empty) => (module.to_owned(), None, true),
                    Some(Slot::Failed(_)) => (module.to_owned(), Some("Failed"), false),
                    // No seat yet (a pane just let go of it, the roster hasn't
                    // remounted it) is loading too, not a final unnamed view —
                    // else the raw module id flashes in the rail/pane title.
                    Some(_) | None => (module.to_owned(), Some("Loading"), false),
                };
                RailRow {
                    module,
                    label,
                    note,
                    empty,
                }
            })
            .collect()
    }

    /// The one reading of a link, against the views this roster lists.
    pub(crate) fn parse_link(&self, link: &str) -> Link {
        parse_link_among(link, &self.rail())
    }
}

/// A node was connected: every seat is asked again of it, and the roster
/// is read so the rail lists what the network runs.
pub fn connected(client: &crate::backend::RpcClient, network: &str, chain: &str) {
    let snapshot = {
        let mut connection = connection().lock().expect("views rpc");
        connection.rev += 1;
        connection.client = Some(client.clone());
        connection.network = network.to_owned();
        connection.chain = chain.to_owned();
        connection.clone()
    };
    spawn_roster_read(snapshot, || {});
}

/// The node moved (a block landed): the roster is read again, and a
/// program whose code changed is loaded again, under a new generation, and
/// swapped in place when ready. One read in flight at a time; a block that
/// lands during one is covered by the next. The read alone holds that
/// latch: the loads it queues land on their own. Cheap when nothing moved.
pub fn deployments_checked() {
    static IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let snapshot = connection().lock().expect("views rpc").clone();
    if snapshot.client.is_none()
        || !spawn_roster_read(snapshot, || IN_FLIGHT.store(false, Ordering::SeqCst))
    {
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

/// [`read_roster`] of the app's roster and seats on a thread of its own,
/// then `then`, even after a read that panicked (the panic hook logs it);
/// `false`, said in app.log, when the OS gives no thread.
fn spawn_roster_read(asked_of: Connection, then: impl FnOnce() + Send + 'static) -> bool {
    let spawned = std::thread::Builder::new()
        .name("roster".into())
        .spawn(move || {
            let read = std::panic::AssertUnwindSafe(|| read_roster(asked_of, roster(), registry()));
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

/// One read of the node's programs into `roster`, and of `registry`'s
/// seats against it: a program that left gives up its seat, a new one is
/// seated (instance 0), and every seat whose code moved, or that was never
/// asked of this node, is queued a load. The read is over once they are
/// queued: nothing waits on a load, which lands in its seat on its own.
pub(super) fn read_roster(asked_of: Connection, roster: &Roster, registry: &Registry) {
    let Some(client) = asked_of.client.as_ref() else {
        return;
    };
    let read = crate::perf::time(crate::perf::Key::Shell, "roster");
    let programs = handle().block_on(crate::backend::views::programs(client, &asked_of.network));
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
            return;
        }
    };
    let roles_due = roster.bound().asked_of != Some(asked_of.rev) || *roster.lock() != programs;
    let roles = if roles_due {
        match handle().block_on(crate::backend::views::roles(client, &asked_of.network)) {
            Ok(roles) => Some(roles),
            // nothing listed either: the next block reads both again
            Err(error) => {
                tracing::warn!(
                    target: "ducktape::app",
                    reason = "roles_unreadable",
                    error = %error,
                    "the node's role bindings were not read"
                );
                return;
            }
        }
    } else {
        None
    };
    let loads = {
        let node_since_left = connection().lock().expect("views rpc").rev != asked_of.rev;
        if node_since_left {
            return;
        }
        if let Some(roles) = roles {
            *roster.bound() = Bound {
                asked_of: Some(asked_of.rev),
                roles,
            };
        }
        let mut registry = lock(registry);
        let names: Vec<&'static str> = programs.iter().map(|p| intern(&p.name)).collect();
        // a program the roster no longer lists gives up its seat
        for gone in registry
            .keys()
            .copied()
            .filter(|(module, _)| !names.contains(module))
            .collect::<Vec<_>>()
        {
            if let Some(retired) = registry.get(&gone) {
                lock(retired).retire();
            }
            if gone.1 == 0 {
                registry.remove(&gone);
            }
        }
        let previous = relist(roster, programs.clone());
        let mut loads = Vec::new();
        for module in &names {
            if !registry.keys().any(|(name, _)| name == module) {
                registry.insert((*module, 0), Mounted::seat());
            }
        }
        for ((module, _), seat) in registry.iter() {
            let Some(program) = programs.iter().find(|program| program.name == *module) else {
                continue;
            };
            let mut locked = lock(seat);
            let same_code = previous
                .iter()
                .any(|old| old.name == program.name && old.code == program.code);
            let asked_of_this_node = locked.generation > 0 && locked.rev == asked_of.rev;
            let code = code_digest(&program.code);
            if !locked.reload_due(same_code, asked_of_this_node, code, Instant::now()) {
                continue;
            }
            locked.rev = asked_of.rev;
            loads.push(Load {
                module,
                seat: seat.clone(),
                generation: locked.start(),
                asked_of: asked_of.clone(),
                code: Some((program.code, program.bare)),
            });
        }
        loads
    };
    // the registry let go first: a load that cannot get a thread installs
    // its failure under the seat's lock
    for load in loads {
        queue(load);
    }
}

/// What a link names, as far as the app can tell without the chain in
/// hand. The `duck://` grammar is shared with the modules: this reads it,
/// it does not change it.
#[derive(Debug, PartialEq)]
pub enum Link {
    /// A view on this connection by its short name (`duck://<view>`, or
    /// `duck://<view>/<route>` with the route its view reads).
    View {
        module: &'static str,
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
    // a short name first: `duck://chat` is also a well-formed chain link
    if let Some(module) = local_seat(link, rows) {
        return Link::View {
            module,
            route: None,
        };
    }
    match ducklink::Link::parse(link) {
        Ok(parsed) => Link::Chain(parsed),
        Err(_) if link.starts_with("http://") || link.starts_with("https://") => {
            Link::Web(link.to_owned())
        }
        Err(_) => match local_seat_route(link, rows) {
            Some((module, route)) => Link::View {
                module,
                route: Some(route),
            },
            None => Link::Unknown,
        },
    }
}

fn local_seat_route(link: &str, rows: &[RailRow]) -> Option<(&'static str, String)> {
    let (seat, route) = link.strip_prefix("duck://")?.split_once('/')?;
    let module = local_seat(&format!("duck://{seat}"), rows)?;
    let route = ducklink::tail(route).ok()?.join("/");
    valid_route(&route).then_some((module, route))
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
    if let Some(row) = rows.iter().find(|row| row.module == name && !row.empty) {
        return Some(row.module);
    }
    let mut matches = rows
        .iter()
        .filter(|row| !row.empty && row.note.is_none() && row.label.to_lowercase() == name);
    let first = matches.next()?;
    matches.next().is_none().then_some(first.module)
}

#[cfg(test)]
mod local_link_tests {
    use super::*;

    #[test]
    fn short_links_resolve_ids_and_unique_ready_labels() {
        let mut rows = vec![RailRow {
            module: "catalog",
            label: "Preferences".into(),
            note: None,
            empty: false,
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
            module: "other",
            label: "Preferences".into(),
            note: None,
            empty: false,
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
            module: "catalog",
            label: "Preferences".into(),
            note: None,
            empty: false,
        }];
        let view = |route: Option<&str>| Link::View {
            module: "catalog",
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

    /// A route is percent-encoded in the link and handed to the view
    /// decoded, whatever it names: a forge room's `:`, a space, Hangul.
    #[test]
    fn a_route_is_delivered_decoded() {
        let rows = [RailRow {
            module: "catalog",
            label: "Preferences".into(),
            note: None,
            empty: false,
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
                    module: "catalog",
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

    /// A module the roster lists but whose seat hasn't been (re)mounted yet
    /// — the gap right after a pane lets go of it and before the next
    /// preload — must not draw as a final, unnamed view: that flashed the
    /// raw module id ("rail-tests-unmounted") in the rail and any popped-out
    /// pane's window title instead of a "Loading" row.
    #[test]
    fn unmounted_module_is_loading_not_a_bare_id() {
        let module = "rail-tests-unmounted";
        // No entry for `module` is inserted into registry(): this is the
        // `None` seat case rail() must treat as loading.
        let row = Roster::listing(&[module])
            .rail()
            .into_iter()
            .find(|row| row.module == module)
            .expect("listed module appears in the rail");
        assert_eq!(row.label, module);
        assert_eq!(row.note, Some("Loading"));
        assert!(!row.empty);
    }

    /// A seat preloaded only as far as compiled is a row ready to open,
    /// named as its manifest names it, as a drawn view's is: the census
    /// opens a view by that name.
    #[test]
    fn a_compiled_seat_is_listed_by_its_manifest_name() {
        let module = "rail-tests-compiled";
        let seat = Mounted::seat();
        lock(&seat).slot = Slot::Compiled {
            name: "Compiled view".into(),
            min_width: 320,
        };
        lock(registry()).insert((module, 0), seat);
        let row = Roster::listing(&[module])
            .rail()
            .into_iter()
            .find(|row| row.module == module)
            .expect("listed module appears in the rail");
        lock(registry()).remove(&(module, 0));
        assert_eq!((row.label.as_str(), row.note), ("Compiled view", None));
        assert!(!row.empty);
    }

    /// A view-only entry (no program behind it) is a rail row like any
    /// program's view, keyed and linked by its own name.
    #[test]
    fn a_view_only_entry_is_a_rail_row_and_a_short_link() {
        let view = abi::BlobId::Sha256([7; 32]);
        let roster = Roster::default();
        roster.lock().push(crate::backend::views::Program {
            name: "explorer".into(),
            code: view,
            bare: true,
        });
        assert!(roster.rail().iter().any(|row| row.module == "explorer"));
        assert_eq!(
            roster.parse_link("duck://explorer"),
            Link::View {
                module: "explorer",
                route: None
            }
        );
        assert_eq!(roster.code("explorer"), Some((view, true)));
    }
}
