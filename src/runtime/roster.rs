use super::*;

// ---------- mounting ----------

// ---------- the roster ----------

/// The session facts every view is handed as its props: the seated `key`
/// (hex) and the `account` it holds, `None` until one is resolved.
pub fn props(
    dark: bool,
    connected: bool,
    network: &str,
    key: &str,
    account: Option<u64>,
    endpoint: &str,
) -> Vec<u8> {
    wire::doors::encode(&wire::doors::Session {
        connected,
        dark,
        chain: network.into(),
        key: key.into(),
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

/// The roster as the node last listed it: every program and its code.
pub(super) fn listed() -> &'static Mutex<Vec<crate::backend::views::Program>> {
    static LISTED: OnceLock<Mutex<Vec<crate::backend::views::Program>>> = OnceLock::new();
    LISTED.get_or_init(Mutex::default)
}

/// The blob a listed module's view comes from, and whether that blob is the
/// view itself (a view-only entry) rather than a program carrying it.
pub(super) fn listed_code(module: &str) -> Option<(abi::BlobId, bool)> {
    listed()
        .lock()
        .expect("roster")
        .iter()
        .find(|program| program.name == module)
        .map(|program| (program.code, program.bare))
}

/// Whether the roster lists `module`: a link to anything else names nothing.
pub fn listed_view(module: &str) -> bool {
    listed_code(module).is_some()
}

#[cfg(test)]
pub(crate) fn list_for_test(module: &str) {
    listed()
        .lock()
        .unwrap()
        .push(crate::backend::views::Program {
            name: module.into(),
            code: abi::BlobId::Sha256([0; 32]),
            bare: false,
        });
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
pub struct RailRow {
    pub module: &'static str,
    pub label: String,
    /// `Some` while the seat is on its way or failed; `None` once drawn.
    pub note: Option<&'static str>,
    /// The program ships no view: the rail leaves it out.
    pub empty: bool,
}

pub fn rail() -> Vec<RailRow> {
    let programs: Vec<&'static str> = listed()
        .lock()
        .expect("roster")
        .iter()
        .map(|program| intern(&program.name))
        .collect();
    let registry = registry().lock().expect("module views");
    programs
        .into_iter()
        .map(|module| {
            let seat = registry
                .iter()
                .find_map(|((name, _), seat)| (*name == module).then_some(seat))
                .map(|seat| seat.lock().expect("module view lock"));
            let (label, note, empty) = match seat.as_ref().map(|seat| &seat.slot) {
                Some(Slot::Ready(guest)) if !guest.name.is_empty() => {
                    (guest.name.clone(), None, false)
                }
                Some(Slot::Ready(_)) => (module.to_owned(), None, false),
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

/// A node was connected: every seat is asked again of it, and the roster
/// is read so the rail lists what the network runs.
pub fn connected(client: &crate::backend::RpcClient, network: &str, chain: &str) -> Loads {
    let snapshot = {
        let mut connection = connection().lock().expect("views rpc");
        connection.rev += 1;
        connection.client = Some(client.clone());
        connection.network = network.to_owned();
        connection.chain = chain.to_owned();
        connection.clone()
    };
    let registry = registry().lock().expect("module views");
    for mounted in registry.values() {
        mounted
            .lock()
            .expect("module view lock")
            .changes
            .send_replace(());
    }
    drop(registry);
    Loads {
        _threads: vec![spawn_roster_read(snapshot)],
    }
}

/// The node moved (a block landed): the roster is read again, and a
/// program whose code changed is loaded again. Cheap when nothing moved.
pub fn deployments_checked() -> Loads {
    static IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Loads {
            _threads: Vec::new(),
        };
    }
    let snapshot = connection().lock().expect("views rpc").clone();
    if snapshot.client.is_none() {
        IN_FLIGHT.store(false, Ordering::SeqCst);
        return Loads {
            _threads: Vec::new(),
        };
    }
    Loads {
        _threads: vec![std::thread::spawn(move || {
            let read = spawn_roster_read(snapshot);
            let _ = read.join();
            IN_FLIGHT.store(false, Ordering::SeqCst);
        })],
    }
}

pub(super) fn spawn_roster_read(asked_of: Connection) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Some(client) = asked_of.client.as_ref() else {
            return;
        };
        let programs =
            match handle().block_on(crate::backend::views::programs(client, &asked_of.network)) {
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
        let loads = {
            let node_since_left = connection().lock().expect("views rpc").rev != asked_of.rev;
            if node_since_left {
                return;
            }
            let mut registry = registry().lock().expect("module views");
            let names: Vec<&'static str> = programs.iter().map(|p| intern(&p.name)).collect();
            // a program the roster no longer lists gives up its seat
            for gone in registry
                .keys()
                .copied()
                .filter(|(module, _)| !names.contains(module))
                .collect::<Vec<_>>()
            {
                if let Some(retired) = registry.get(&gone) {
                    let mut retired = retired.lock().expect("module view lock");
                    retired.generation += 1;
                    retired.slot = Slot::Empty;
                    retired.changes.send_replace(());
                }
                if gone.1 == 0 {
                    registry.remove(&gone);
                }
            }
            let previous =
                std::mem::replace(&mut *listed().lock().expect("roster"), programs.clone());
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
                let mut locked = seat.lock().expect("module view lock");
                let same_code = previous
                    .iter()
                    .any(|old| old.name == program.name && old.code == program.code);
                let asked_of_this_node = locked.generation > 0 && locked.rev == asked_of.rev;
                if same_code && asked_of_this_node && !locked.held_off_now() {
                    continue;
                }
                if locked.held_off(Some(code_digest(&program.code))) {
                    continue;
                }
                locked.rev = asked_of.rev;
                let generation = locked.start();
                drop(locked);
                loads.push(spawn_load(module, seat, generation, asked_of.clone()));
            }
            loads
        };
        for load in loads {
            let _ = load.join();
        }
    })
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

/// The one reading of a link, against the views this connection lists.
pub fn parse_link(link: &str) -> Link {
    parse_link_among(link, &rail())
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
        listed()
            .lock()
            .unwrap()
            .push(crate::backend::views::Program {
                name: module.into(),
                code: abi::BlobId::Sha256([0; 32]),
                bare: false,
            });
        // No entry for `module` is inserted into registry(): this is the
        // `None` seat case rail() must treat as loading.
        let row = rail()
            .into_iter()
            .find(|row| row.module == module)
            .expect("listed module appears in the rail");
        assert_eq!(row.label, module);
        assert_eq!(row.note, Some("Loading"));
        assert!(!row.empty);
    }

    /// A view-only entry (no program behind it) is a rail row like any
    /// program's view, keyed and linked by its own name.
    #[test]
    fn a_view_only_entry_is_a_rail_row_and_a_short_link() {
        let view = abi::BlobId::Sha256([7; 32]);
        listed()
            .lock()
            .unwrap()
            .push(crate::backend::views::Program {
                name: "explorer".into(),
                code: view,
                bare: true,
            });
        assert!(rail().iter().any(|row| row.module == "explorer"));
        assert_eq!(
            parse_link("duck://explorer"),
            Link::View {
                module: "explorer",
                route: None
            }
        );
        assert_eq!(listed_code("explorer"), Some((view, true)));
    }
}
