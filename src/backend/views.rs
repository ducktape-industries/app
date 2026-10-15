//! Where a view comes from: the connected node's roster. The programs
//! role names every program and the code blob it runs; a program that ships a
//! view carries it inside that blob, as the custom section
//! [`VIEW_SECTION`] — one artifact, one blob id. The app reads the section
//! out and mounts it. A program without the section has no view, which is
//! the network's fact, not a failure. The programs role also lists view-only
//! entries: a name and a blob that is the view itself, with no program
//! behind it (Explorer); they follow the programs.
//!
//! Nothing here knows a program by name: the roster's order (programs by
//! name, then view-only entries by name, as the programs role answers them) is the
//! rail's order, the manifest inside the view names the tab.

use std::path::PathBuf;

use abi::BlobId;

use super::{RpcClient, cache_dir};
use noded_client::wire::{Layer, unframe};

pub const VIEW_SECTION: &str = "ducktape.view";

/// The most a program's code blob may be, framed: the view inside it is
/// compiled, so no view is near it.
const MAX_PROGRAM_BYTES: usize = 64 << 20;

/// The most roster entries the app takes: a node that lists more has the
/// rest left off the rail and never loaded, said once in app.log.
pub const MAX_PROGRAMS: usize = 256;

/// One roster entry, as the rail lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub name: String,
    pub code: BlobId,
    /// `code` is the view itself, with no program behind it.
    pub bare: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// The node could not be asked.
    Unreachable(String),
    /// The node does not hold the bytes (yet).
    NotHeld,
    /// The bytes came, and do not hash to the id asked for.
    Corrupt,
    Collision(BlobId),
    /// The node answered something this app does not read.
    Refused(String),
}

impl std::fmt::Display for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fetch::Unreachable(reason) | Fetch::Refused(reason) => f.write_str(reason),
            Fetch::NotHeld => f.write_str("the node does not hold the program's code yet"),
            Fetch::Corrupt => f.write_str("the program's bytes do not hash to its blob id"),
            Fetch::Collision(code) => write!(
                f,
                "the program's bytes named {code:?} carry a SHA-1 collision attack"
            ),
        }
    }
}

/// The roster as the programs role answers it at the node's height
/// (sorted by name, programs first): asked of that program through the
/// same query path every view's request takes. Only an id in its one
/// spelling (`program::is_name`) is taken: the chain accepts more (a
/// capital, a space, a newline), and such an id could read as another's
/// wherever the app names it (the rail, a pane, the consent card's
/// "Program <id>", which a newline would end before a "System asks" of its
/// own). One left off is said in app.log, once while the same ids are.
pub async fn programs(client: &RpcClient, network: &str) -> Result<Vec<Program>, Fetch> {
    use ::programs::{Query, Reply};
    let Reply::Roster {
        programs, views, ..
    } = ask(client, network, Query::Roster).await?
    else {
        return Err(Fetch::Refused(
            "the programs role answered no roster".into(),
        ));
    };
    let programs = programs.into_iter().map(|entry| Program {
        name: entry.program,
        code: entry.code,
        bare: false,
    });
    let views = views.into_iter().map(|view| Program {
        name: view.name,
        code: view.view,
        bare: true,
    });
    let (mut roster, misspelled): (Vec<Program>, Vec<Program>) = programs
        .chain(views)
        .partition(|program| program::is_name(&program.name));
    misspelled_said(misspelled.into_iter().map(|program| program.name).collect());
    if roster.len() > MAX_PROGRAMS {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            tracing::warn!(
                target: "ducktape::app",
                reason = "roster_capped",
                listed = roster.len(),
                kept = MAX_PROGRAMS,
                "the node lists more programs than the app takes"
            );
        });
        roster.truncate(MAX_PROGRAMS);
    }
    Ok(roster)
}

/// Warns of the ids left off the roster, escaped (`{:?}`), when they are
/// not the ones the last read left off: once, not every block.
fn misspelled_said(ids: Vec<String>) {
    static LAST: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if *last != ids {
        if !ids.is_empty() {
            tracing::warn!(
                target: "ducktape::app",
                reason = "program_id_misspelled",
                ids = ?ids,
                "the node lists program ids outside 1..=64 of [a-z0-9_-]; they are left off the roster"
            );
        }
        *last = ids;
    }
}

/// Which programs fill the roles the kernel calls, as genesis bound them
/// and the programs role answers them. `None` from a programs role that
/// predates the question, which refuses it: no program is known by its role.
pub async fn roles(client: &RpcClient, network: &str) -> Result<Option<abi::Roles>, Fetch> {
    use ::programs::{Query, Reply};
    match ask(client, network, Query::Roles).await {
        Ok(Reply::Roles(roles)) => Ok(Some(roles)),
        Ok(_) | Err(Fetch::Refused(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn ask(
    client: &RpcClient,
    network: &str,
    query: ::programs::Query,
) -> Result<::programs::Reply, Fetch> {
    let frame = super::query_frame(network, ::programs::PROGRAM, abi::encode(&query)).await;
    let answer = client
        .query(Layer::Preconfirmed, frame)
        .await
        .map_err(fetch_error)?;
    abi::decode(&answer).map_err(|refusal| Fetch::Refused(refusal.sentence))
}

/// The view inside `code`'s blob, or `None` when the program ships none; a
/// `bare` blob is the view itself. A blob read once is kept in the cache
/// directory under its id; the word beside the bytes says where they came
/// from, `disk` or `node`.
pub async fn view_of(
    client: &RpcClient,
    code: &BlobId,
    bare: bool,
) -> Result<Option<(Vec<u8>, &'static str)>, Fetch> {
    let (bytes, source) = program_bytes(client, code).await?;
    Ok(if bare {
        Some((bytes, source))
    } else {
        view_section(&bytes).map(|view| (view, source))
    })
}

pub async fn program_bytes(
    client: &RpcClient,
    code: &BlobId,
) -> Result<(Vec<u8>, &'static str), Fetch> {
    let cached = cache_path(code);
    if let Some(path) = &cached
        && let Ok(framed) = tokio::fs::read(path).await
        && hashes_to(&framed, code)?
        && let Some(body) = unframe(&framed)
    {
        return Ok((body.to_vec(), "disk"));
    }
    let framed = client
        .blob(*code, MAX_PROGRAM_BYTES)
        .await
        .map_err(fetch_error)?
        .ok_or(Fetch::NotHeld)?;
    let body = unframe(&framed)
        .ok_or_else(|| Fetch::Refused("the blob carries no git header".into()))?
        .to_vec();
    if !hashes_to(&framed, code)? {
        return Err(Fetch::Corrupt);
    }
    if let Some(path) = cached {
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(path, &framed).await;
    }
    Ok((body, "node"))
}

fn cache_path(code: &BlobId) -> Option<PathBuf> {
    Some(
        cache_dir()
            .ok()?
            .join("programs")
            .join(abi::hex(code.digest())),
    )
}

/// A blob id names the FRAMED bytes (`kind len\0body`), as git's does, and
/// the cache keeps them framed: the kind is the node's word (a program is
/// `program`), which the id alone cannot give back.
fn hashes_to(framed: &[u8], code: &BlobId) -> Result<bool, Fetch> {
    let hashed = blob::id_of(code.kind(), framed).map_err(|_| Fetch::Collision(*code))?;
    Ok(hashed == *code)
}

/// The bytes of the [`VIEW_SECTION`] custom section, out of a core module or
/// a component (where the section may sit on a nested core module).
pub fn view_section(bytes: &[u8]) -> Option<Vec<u8>> {
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let Ok(wasmparser::Payload::CustomSection(section)) = payload
            && section.name() == VIEW_SECTION
        {
            return Some(section.data().to_vec());
        }
    }
    None
}

fn fetch_error(error: noded_client::Error) -> Fetch {
    match error {
        noded_client::Error::Refused(refusal) | noded_client::Error::Decode(refusal) => {
            Fetch::Refused(refusal.sentence)
        }
        error @ noded_client::Error::TooLarge { .. } => Fetch::Refused(error.to_string()),
        other => Fetch::Unreachable(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_section_is_read_out_of_a_core_module() {
        // a minimal core module: magic, version, one custom section
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        let name = VIEW_SECTION.as_bytes();
        let body = b"view bytes";
        let mut section = vec![name.len() as u8];
        section.extend_from_slice(name);
        section.extend_from_slice(body);
        module.push(0);
        module.push(section.len() as u8);
        module.extend_from_slice(&section);
        assert_eq!(view_section(&module).as_deref(), Some(&body[..]));
        assert_eq!(view_section(b"\0asm\x01\0\0\0"), None);
    }

    #[test]
    fn a_sha1_code_hashes_as_git_does_and_a_shattered_half_is_refused_by_its_id() {
        let hello =
            noded_client::hex::hex_decode("ce013625030ba8dba906f756967f9e9ca394464a").unwrap();
        let hello = BlobId::Sha1(hello.try_into().unwrap());
        assert!(hashes_to(b"blob 6\0hello\n", &hello).unwrap());
        assert!(!hashes_to(b"blob 6\0hullo\n", &hello).unwrap());
        let shared =
            noded_client::hex::hex_decode("f92d74e3874587aaf443d1db961d4e26dde13e9c").unwrap();
        let code = BlobId::Sha1(shared.try_into().unwrap());
        for half in [
            &include_bytes!("fixtures/shattered-1.bin")[..],
            &include_bytes!("fixtures/shattered-2.bin")[..],
        ] {
            let refused = hashes_to(half, &code).unwrap_err();
            assert!(matches!(refused, Fetch::Collision(named) if named == code));
            assert!(
                refused
                    .to_string()
                    .contains("f92d74e3874587aaf443d1db961d4e26dde13e9c")
            );
        }
    }
}
