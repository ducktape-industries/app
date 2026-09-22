use super::*;

// ---------- mounting ----------

// ---------- the roster ----------

/// The session facts every view is handed as its props.
pub fn props(dark: bool, connected: bool, network: &str, account: &str, endpoint: &str) -> Vec<u8> {
    wire::doors::encode(&wire::doors::Session {
        connected,
        dark,
        chain: network.into(),
        account: account.into(),
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

pub(super) fn listed_code(module: &str) -> Option<abi::BlobId> {
    listed()
        .lock()
        .expect("roster")
        .iter()
        .find(|program| program.name == module)
        .map(|program| program.code)
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
                Some(Slot::Ready(_)) | None => (module.to_owned(), None, false),
                Some(Slot::Empty) => (module.to_owned(), None, true),
                Some(Slot::Failed(_)) => (module.to_owned(), Some("Failed"), false),
                Some(_) => (module.to_owned(), Some("Loading"), false),
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
pub fn connected(client: &crate::backend::RpcClient, network: &str) -> Loads {
    let snapshot = {
        let mut connection = connection().lock().expect("views rpc");
        connection.rev += 1;
        connection.client = Some(client.clone());
        connection.network = network.to_owned();
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
    Loads(vec![spawn_roster_read(snapshot)])
}

/// The node moved (a block landed): the roster is read again, and a
/// program whose code changed is loaded again. Cheap when nothing moved.
pub fn deployments_checked() -> Loads {
    static IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Loads(Vec::new());
    }
    let snapshot = connection().lock().expect("views rpc").clone();
    if snapshot.client.is_none() {
        IN_FLIGHT.store(false, Ordering::SeqCst);
        return Loads(Vec::new());
    }
    Loads(vec![std::thread::spawn(move || {
        let read = spawn_roster_read(snapshot);
        let _ = read.join();
        IN_FLIGHT.store(false, Ordering::SeqCst);
    })])
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

/// A short link names a program or a unique view label on this connection.
pub fn local_link(link: &str) -> Option<&'static str> {
    local_seat(link, &rail())
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
}
