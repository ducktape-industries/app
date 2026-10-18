//! Where a view comes from: the connected node's roster. The programs
//! role names every program and the code blob it runs; a program that ships a
//! view carries it inside that blob, as the custom section
//! [`VIEW_SECTION`] — one artifact, one blob id. The app reads the section
//! out and mounts it. A program without the section has no view, which is
//! the network's fact, not a failure. The programs role also lists view-only
//! entries: a name and a blob that is the view itself, with no program
//! behind it (Explorer); they follow the programs.
//!
//! A program may also carry its NOTIFIER, the headless module that decides
//! its notices, as the custom section [`NOTIFY_SECTION`] of the same blob
//! (`runtime::notifier` runs it).
//!
//! Nothing here knows a program by name: the roster's order (programs by
//! name, then view-only entries by name, as the programs role answers them) is the
//! rail's order, the manifest inside the view names the tab.

use std::path::{Path, PathBuf};

use abi::BlobId;

use super::RpcClient;
use noded_client::wire::{Layer, unframe};

pub const VIEW_SECTION: &str = "ducktape.view";
pub const NOTIFY_SECTION: &str = "ducktape.notify";

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
    Refused(String),
    Unreadable(String),
}

impl std::fmt::Display for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fetch::Unreachable(reason) | Fetch::Refused(reason) | Fetch::Unreadable(reason) => {
                f.write_str(reason)
            }
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
        return Err(Fetch::Unreadable(
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
    let (roster, misspelled): (Vec<Program>, Vec<Program>) = programs
        .chain(views)
        .partition(|program| program::is_name(&program.name));
    misspelled_said(misspelled.into_iter().map(|program| program.name).collect());
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
/// and the programs role answers them.
pub async fn roles(client: &RpcClient, network: &str) -> Result<abi::Roles, Fetch> {
    use ::programs::{Query, Reply};
    match ask(client, network, Query::Roles).await? {
        Reply::Roles(roles) => Ok(roles),
        _ => Err(Fetch::Unreadable(
            "the programs role answered the roles question with another reply".into(),
        )),
    }
}

/// The code every scheduled change brings (a program set, a view set), in
/// the programs role's order, whatever height it lands at: what the
/// network is about to run, so a view may ask about it (`blob.manifest`)
/// before it does. Read for that ask, not per block. Empty from a programs
/// role that predates the question.
pub async fn coming(client: &RpcClient, network: &str) -> Result<Vec<BlobId>, Fetch> {
    use ::programs::{PageRequest, Query, Reply};
    let mut codes = Vec::new();
    let mut after = None;
    loop {
        let page = PageRequest { after, limit: None };
        let page = match ask(client, network, Query::Scheduled { page }).await {
            Ok(Reply::Scheduled(page)) => page,
            Ok(_) => {
                return Err(Fetch::Unreadable(
                    "the programs role answered the scheduled question with another reply".into(),
                ));
            }
            Err(Fetch::Refused(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        codes.extend(
            page.items
                .iter()
                .filter_map(|pending| pending.change.code()),
        );
        let Some(next) = page.next else {
            return Ok(codes);
        };
        after = Some(next);
    }
}

async fn ask(
    client: &RpcClient,
    network: &str,
    query: ::programs::Query,
) -> Result<::programs::Reply, Fetch> {
    let frame = super::query_frame(network, ::programs::PROGRAM, abi::encode(&query))
        .await
        .map_err(|refusal| Fetch::Refused(refusal.message))?;
    let answer = client
        .query(Layer::Preconfirmed, frame)
        .await
        .map_err(fetch_error)?;
    abi::decode(&answer).map_err(|refusal| Fetch::Unreadable(refusal.sentence))
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

/// A program's blob: the cache's copy, else the node's, which the cache
/// keeps from then on. Only code the roster names is read here (a seat's,
/// `program.describe`'s, `blob.manifest`'s), so the cache holds at most
/// what the network runs or has scheduled.
pub async fn program_bytes(
    client: &RpcClient,
    code: &BlobId,
) -> Result<(Vec<u8>, &'static str), Fetch> {
    let cached = cache_path(code);
    if let Some(path) = &cached
        && let Ok(mut framed) = tokio::fs::read(path).await
        && hashes_to(&framed, code)?
        && let Some(body) = body_at(&framed)
    {
        framed.drain(..body);
        return Ok((framed, "disk"));
    }
    let mut framed = client
        .blob(*code)
        .await
        .map_err(fetch_error)?
        .ok_or(Fetch::NotHeld)?;
    let body = body_at(&framed)
        .ok_or_else(|| Fetch::Unreadable("the blob carries no git header".into()))?;
    if !hashes_to(&framed, code)? {
        return Err(Fetch::Corrupt);
    }
    // written whole or not at all (a sibling of its own, renamed into
    // place): a reader never meets half a blob
    if let Some(path) = cached {
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let kept = path.with_extension(format!("{n}.partial"));
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if tokio::fs::write(&kept, &framed).await.is_err()
            || tokio::fs::rename(&kept, &path).await.is_err()
        {
            let _ = tokio::fs::remove_file(&kept).await;
        }
    }
    framed.drain(..body);
    Ok((framed, "node"))
}

/// Where `framed`'s body starts, past its `kind len\0` header: the header
/// is cut off in place, as a copy of the body would hold a blob (megabytes)
/// twice at once.
fn body_at(framed: &[u8]) -> Option<usize> {
    unframe(framed).map(|body| framed.len() - body.len())
}

/// Where the cache keeps `code`'s framed bytes.
pub(crate) fn cache_path(code: &BlobId) -> Option<PathBuf> {
    Some(
        super::dirs()
            .cache
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

/// The bytes of the [`VIEW_SECTION`] custom section ([`section`]).
pub fn view_section(bytes: &[u8]) -> Option<Vec<u8>> {
    section(bytes, VIEW_SECTION).map(<[u8]>::to_vec)
}

/// The notifier inside `code`'s blob, or `None` when the program ships
/// none (a view-only entry never does). Every listed program's blob is
/// asked, to find the few that carry one, and a blob is megabytes, most of
/// it the view: the cache's copy is read through in pieces, the notifier
/// alone kept ([`cached_section`]).
pub async fn notifier_of(client: &RpcClient, code: &BlobId) -> Result<Option<Vec<u8>>, Fetch> {
    if let Some(path) = cache_path(code) {
        let id = *code;
        let cached =
            tokio::task::spawn_blocking(move || cached_section(&path, &id, NOTIFY_SECTION));
        if let Ok(Some(notifier)) = cached.await {
            return Ok(notifier);
        }
    }
    let (bytes, _) = program_bytes(client, code).await?;
    Ok(section(&bytes, NOTIFY_SECTION).map(<[u8]>::to_vec))
}

/// The custom section `name` of the cache's copy of `code` (`Some(None)`:
/// the blob carries none), its bytes hashed as they pass and the section
/// alone kept. `None` when the cache holds no copy that hashes to `code`,
/// or the blob is not one core module (a component's sections sit on its
/// nested modules: [`section`] reads those).
fn cached_section(path: &Path, code: &BlobId, name: &str) -> Option<Option<Vec<u8>>> {
    use std::io::Read as _;
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut read = Hashed {
        read: file,
        hasher: blob::Hasher::new(code.kind()),
        at: 0,
    };
    // the `kind len\0` header, then a core module's preamble
    while read.byte()? != 0 {}
    let mut preamble = [0; 8];
    read.read_exact(&mut preamble).ok()?;
    // ponytail: a component's notifier is still read whole; walk its
    // nested modules here if a program ships one as a component
    if preamble != *b"\0asm\x01\0\0\0" {
        return None;
    }
    let mut found = None;
    while let Some(id) = read.byte() {
        let size = read.leb()?;
        let end = read.at + size;
        if id == 0 && found.is_none() && read.leb()? == name.len() as u64 {
            let mut named = vec![0; name.len()];
            read.read_exact(&mut named).ok()?;
            if named == name.as_bytes() {
                let mut section = Vec::new();
                let left = end.checked_sub(read.at)?;
                (&mut read).take(left).read_to_end(&mut section).ok()?;
                found = Some(section);
            }
        }
        let left = end.checked_sub(read.at)?;
        std::io::copy(&mut (&mut read).take(left), &mut std::io::sink()).ok()?;
    }
    (read.hasher.finish().ok()? == *code).then_some(found)
}

/// A read that hashes what passes through it, and counts it.
struct Hashed<R> {
    read: R,
    hasher: blob::Hasher,
    at: u64,
}

impl<R: std::io::Read> std::io::Read for Hashed<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.read.read(buf)?;
        self.hasher.update(&buf[..read]);
        self.at += read as u64;
        Ok(read)
    }
}

impl<R: std::io::Read> Hashed<R> {
    /// The next byte; none at the end.
    fn byte(&mut self) -> Option<u8> {
        let mut byte = [0];
        std::io::Read::read_exact(self, &mut byte).ok()?;
        Some(byte[0])
    }

    /// A wasm LEB128 `u32`.
    fn leb(&mut self) -> Option<u64> {
        let mut value = 0;
        for shift in (0..35).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(value);
            }
        }
        None
    }
}

/// The bytes of the custom section `name`, out of a core module or a
/// component (where the section may sit on a nested core module).
pub fn section<'a>(bytes: &'a [u8], name: &str) -> Option<&'a [u8]> {
    wasmparser::Parser::new(0)
        .parse_all(bytes)
        .find_map(|payload| match payload {
            Ok(wasmparser::Payload::CustomSection(section)) if section.name() == name => {
                Some(section.data())
            }
            _ => None,
        })
}

fn fetch_error(error: noded_client::Error) -> Fetch {
    match error {
        noded_client::Error::Refused(refusal) => Fetch::Refused(refusal.sentence),
        noded_client::Error::Decode(refusal) => Fetch::Unreadable(refusal.sentence),
        other => Fetch::Unreachable(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The notifier is read out of the cache's copy of its program in one
    /// pass, the view before it never held whole; a copy that does not hash
    /// to its id, or a component, is left to the whole read.
    #[test]
    fn the_notifier_alone_is_read_out_of_the_cached_blob() {
        fn leb(mut n: usize) -> Vec<u8> {
            let mut bytes = Vec::new();
            loop {
                let byte = (n & 0x7f) as u8;
                n >>= 7;
                if n == 0 {
                    bytes.push(byte);
                    return bytes;
                }
                bytes.push(byte | 0x80);
            }
        }
        let custom = |name: &str, data: &[u8]| {
            let mut content = leb(name.len());
            content.extend_from_slice(name.as_bytes());
            content.extend_from_slice(data);
            [vec![0], leb(content.len()), content].concat()
        };
        // a type section, a view past one LEB byte, then the notifier
        let body = [
            b"\0asm\x01\0\0\0".to_vec(),
            vec![1, 4, 1, 0x60, 0, 0],
            custom(VIEW_SECTION, &[7; 300]),
            custom(NOTIFY_SECTION, b"the notifier"),
        ]
        .concat();
        let framed = |body: &[u8]| [format!("program {}\0", body.len()).as_bytes(), body].concat();
        let id = |framed: &[u8]| {
            use sha2::Digest as _;
            BlobId::Sha256(sha2::Sha256::digest(framed).into())
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        let kept = framed(&body);
        std::fs::write(&path, &kept).unwrap();
        let code = id(&kept);
        let read = cached_section(&path, &code, NOTIFY_SECTION);
        assert_eq!(read, Some(Some(b"the notifier".to_vec())));
        assert_eq!(section(&body, NOTIFY_SECTION), Some(&b"the notifier"[..]));
        assert_eq!(cached_section(&path, &code, "ducktape.none"), Some(None));
        let mut corrupt = kept.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        std::fs::write(&path, &corrupt).unwrap();
        assert_eq!(cached_section(&path, &code, NOTIFY_SECTION), None);
        let component = framed(b"\0asm\x0d\0\x01\0");
        std::fs::write(&path, &component).unwrap();
        assert_eq!(cached_section(&path, &id(&component), NOTIFY_SECTION), None);
    }

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
