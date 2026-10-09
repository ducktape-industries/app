//! Where a view comes from: the connected node's roster. The registry
//! names every program and the code blob it runs; a program that ships a
//! view carries it inside that blob, as the custom section
//! [`VIEW_SECTION`] — one artifact, one blob id. The app reads the section
//! out and mounts it. A program without the section has no view, which is
//! the network's fact, not a failure. The registry also lists view-only
//! entries: a name and a blob that is the view itself, with no program
//! behind it (Explorer); they follow the programs.
//!
//! Nothing here knows a program by name: the roster's order (programs by
//! name, then view-only entries by name, as the registry folds them) is the
//! rail's order, the manifest inside the view names the tab.

use std::path::PathBuf;

use abi::BlobId;

use super::noded::Layer;
use super::{RpcClient, cache_dir};

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
    /// The node answered something this app does not read.
    Refused(String),
}

impl std::fmt::Display for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fetch::Unreachable(reason) | Fetch::Refused(reason) => f.write_str(reason),
            Fetch::NotHeld => f.write_str("the node does not hold the program's code yet"),
            Fetch::Corrupt => f.write_str("the program's bytes do not hash to its blob id"),
        }
    }
}

/// The roster as the registry program answers it (sorted by name, programs
/// first): asked of that program through the same query path every view's
/// request takes. Only an id in its one spelling (`program::is_name`) is
/// taken: the chain accepts more (a capital, a space, a newline), and such
/// an id could read as another's wherever the app names it (the rail, a
/// pane, the consent card's "Program <id>", which a newline would end
/// before a "System asks" of its own). One left off is said in app.log,
/// once while the same ids are.
pub async fn programs(client: &RpcClient, network: &str) -> Result<Vec<Program>, Fetch> {
    use module_registry::{Query, Reply};
    let Reply::Programs(entries) = ask(client, network, Query::At(0)).await? else {
        return Err(Fetch::Refused("the registry answered no programs".into()));
    };
    // a registry from before view-only entries refuses the question: no views
    let views = match ask(client, network, Query::Views(0)).await {
        Ok(Reply::Views(views)) => views,
        Ok(_) | Err(Fetch::Refused(_)) => Vec::new(),
        Err(error) => return Err(error),
    };
    let programs = entries.into_iter().map(|entry| Program {
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
/// and the registry program answers them. `None` from a registry from
/// before the question, which refuses it: no program is known by its role.
pub async fn roles(client: &RpcClient, network: &str) -> Result<Option<abi::Roles>, Fetch> {
    use module_registry::{Query, Reply};
    match ask(client, network, Query::Roles).await {
        Ok(Reply::Roles(roles)) => Ok(Some(roles)),
        Ok(_) | Err(Fetch::Refused(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn ask(
    client: &RpcClient,
    network: &str,
    query: module_registry::Query,
) -> Result<module_registry::Reply, Fetch> {
    let frame = super::query_frame(network, module_registry::MODULE, abi::encode(&query)).await;
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
        && let Ok(bytes) = tokio::fs::read(path).await
        && hashes_to(&bytes, code)
    {
        return Ok((bytes, "disk"));
    }
    let framed = client
        .blob(*code, MAX_PROGRAM_BYTES)
        .await
        .map_err(fetch_error)?
        .ok_or(Fetch::NotHeld)?;
    let body = super::noded::unframe(&framed)
        .ok_or_else(|| Fetch::Refused("the blob carries no git header".into()))?
        .to_vec();
    if !hashes_to(&framed, code) {
        return Err(Fetch::Corrupt);
    }
    if let Some(path) = cached {
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(path, &body).await;
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

/// A blob id names the FRAMED bytes (`kind len\0body`), as git's does; the
/// cache holds the body, which is re-framed as a blob before the check.
fn hashes_to(bytes: &[u8], code: &BlobId) -> bool {
    use sha2::Digest as _;
    // The cache holds a bare body, the store a git-framed blob; a body is
    // not told apart by its bytes (a wasm body opens with a NUL, exactly
    // where a frame ends), so both readings are hashed and either may match.
    let mut framed = format!("blob {}\0", bytes.len()).into_bytes();
    framed.extend_from_slice(bytes);
    let matches = |candidate: &[u8]| match code.kind() {
        abi::HashKind::Sha1 => sha1::Sha1::digest(candidate)[..] == *code.digest(),
        abi::HashKind::Sha256 => sha2::Sha256::digest(candidate)[..] == *code.digest(),
    };
    matches(bytes) || matches(&framed)
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

fn fetch_error(error: super::noded::Error) -> Fetch {
    match error {
        super::noded::Error::Refused(refusal) | super::noded::Error::Decode(refusal) => {
            Fetch::Refused(refusal.sentence)
        }
        error @ super::noded::Error::TooLarge { .. } => Fetch::Refused(error.to_string()),
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

    /// A cached body hashes to the framed blob's id, framed or bare; a
    /// wasm body, which opens with a NUL, must not pass for a framed one.
    #[test]
    fn a_cached_body_hashes_like_the_framed_blob() {
        use sha2::Digest as _;
        for body in [&b"hello"[..], b"\0asm\x01\0\0\0"] {
            let mut framed = format!("blob {}\0", body.len()).into_bytes();
            framed.extend_from_slice(body);
            let id = BlobId::Sha256(sha2::Sha256::digest(&framed).into());
            assert!(hashes_to(body, &id), "{body:?}");
            assert!(hashes_to(&framed, &id), "{body:?}");
            assert!(!hashes_to(b"other", &id));
        }
    }
}
