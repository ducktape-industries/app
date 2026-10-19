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
use crate::contract::Code;
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
pub async fn programs(
    client: &RpcClient,
    network: &str,
    signer: &dyn noded_client::Signer,
) -> Result<Vec<Program>, Fetch> {
    use ::programs::{Query, Reply};
    let Reply::Roster {
        programs, views, ..
    } = ask(client, network, signer, Query::Roster).await?
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
    misspelled_said(
        network,
        misspelled.into_iter().map(|program| program.name).collect(),
    );
    Ok(roster)
}

/// Warns of the ids left off `network`'s roster, escaped (`{:?}`), when
/// they are not the ones its last read left off: once, not every block,
/// with another listed network's roster read in turn.
fn misspelled_said(network: &str, ids: Vec<String>) {
    static LAST: std::sync::Mutex<std::collections::BTreeMap<String, Vec<String>>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let last = last.entry(network.to_owned()).or_default();
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
pub async fn roles(
    client: &RpcClient,
    network: &str,
    signer: &dyn noded_client::Signer,
) -> Result<abi::Roles, Fetch> {
    use ::programs::{Query, Reply};
    match ask(client, network, signer, Query::Roles).await? {
        Reply::Roles(roles) => Ok(roles),
        _ => Err(Fetch::Unreadable(
            "the programs role answered the roles question with another reply".into(),
        )),
    }
}

async fn ask(
    client: &RpcClient,
    network: &str,
    signer: &dyn noded_client::Signer,
    query: ::programs::Query,
) -> Result<::programs::Reply, Fetch> {
    let frame = super::query_frame(signer, network, ::programs::PROGRAM, abi::encode(&query))
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
) -> Result<Option<(Blob, &'static str)>, Fetch> {
    let (blob, source) = program_bytes(client, code).await?;
    Ok(match bare {
        true => Some((blob, source)),
        false => blob
            .part(|body| section(body, VIEW_SECTION))
            .map(|view| (view, source)),
    })
}

/// A program's blob, or a section of it, as this process holds it: the
/// cache's file mapped, or bytes in memory (the node's, when the cache
/// cannot keep them; a bundle's). A blob is megabytes. Read into the heap
/// it is a block glibc keeps in a thread's arena once freed, and the free
/// of a block that size raises glibc's thresholds for giving memory back
/// (mmap's to the block's size, trim's to twice that) for the whole
/// process, for good. A map is the file's own pages, given back whole when
/// it goes.
pub struct Blob {
    held: Held,
    /// Where in `held` this blob, or this part of one, lies.
    range: std::ops::Range<usize>,
}

enum Held {
    /// The cache's file at the path, mapped whole.
    Mapped(memmap2::Mmap, PathBuf),
    /// A file no path names, mapped whole ([`unkept_bytes`]).
    Unnamed(memmap2::Mmap),
    /// Bytes in memory.
    Read(Vec<u8>),
}

impl Blob {
    /// The file at `path`, mapped whole.
    fn mapped(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        // SAFETY: a map's bytes change under it only when its file is
        // written in place, and the cache's files never are: `fetched`
        // writes each one whole under a name of its own and renames it
        // into place, and nothing writes it after. A rename over it or a
        // removal leaves the map whole.
        let map = unsafe { memmap2::Mmap::map(&file) }?;
        Ok(Self {
            range: 0..map.len(),
            held: Held::Mapped(map, path.to_owned()),
        })
    }

    /// `file`, which no path names, mapped whole.
    fn unnamed(file: &std::fs::File) -> std::io::Result<Self> {
        // SAFETY: no path names the file, and [`unkept_bytes`], which made
        // it, wrote it whole before the map and holds it nowhere after.
        let map = unsafe { memmap2::Mmap::map(file) }?;
        Ok(Self {
            range: 0..map.len(),
            held: Held::Unnamed(map),
        })
    }

    /// The part of these bytes `part` reads out of them, as a blob of its
    /// own: nothing is copied. `None` when it reads none, or answers bytes
    /// that are not these.
    pub fn part(self, part: impl FnOnce(&[u8]) -> Option<&[u8]>) -> Option<Self> {
        let within = {
            let bytes: &[u8] = &self;
            let part = part(bytes)?;
            let start = (part.as_ptr() as usize).wrapping_sub(bytes.as_ptr() as usize);
            let end = start.checked_add(part.len())?;
            (end <= bytes.len()).then_some(start..end)?
        };
        Some(Self {
            range: self.range.start + within.start..self.range.start + within.end,
            held: self.held,
        })
    }

    /// The bytes as a runner takes them: with where they lie in the
    /// cache's file when they are its, for a compile apart to read there.
    pub fn code(&self) -> Code<'_> {
        Code {
            bytes: self,
            file: match &self.held {
                Held::Mapped(_, path) => Some((path, self.range.start as u64)),
                Held::Unnamed(_) | Held::Read(_) => None,
            },
        }
    }
}

impl From<Vec<u8>> for Blob {
    fn from(bytes: Vec<u8>) -> Self {
        Self {
            range: 0..bytes.len(),
            held: Held::Read(bytes),
        }
    }
}

impl std::ops::Deref for Blob {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match &self.held {
            Held::Mapped(map, _) | Held::Unnamed(map) => &map[self.range.clone()],
            Held::Read(bytes) => &bytes[self.range.clone()],
        }
    }
}

/// A program's blob, its body what derefs: the cache's copy, else the
/// node's, streamed into the cache ([`fetched`]) and mapped there, or read
/// whole off the node when the cache cannot keep it (a full disk). Only
/// code the roster names is read here (a seat's, `program.describe`'s,
/// `blob.manifest`'s), so the cache holds at most what the network runs or
/// has scheduled.
pub async fn program_bytes(
    client: &RpcClient,
    code: &BlobId,
) -> Result<(Blob, &'static str), Fetch> {
    match cached(client, code).await {
        Ok(cached) => cached,
        Err(error) => {
            tracing::warn!(
                target: "ducktape::app",
                reason = "program_code_unkept",
                %error,
                "the cache could not keep a program's code; it is read whole from the node"
            );
            let framed = client
                .blob(*code)
                .await
                .map_err(fetch_error)?
                .ok_or(Fetch::NotHeld)?;
            if !hashes_to(&framed, code)? {
                return Err(Fetch::Corrupt);
            }
            let body = Blob::from(framed).part(unframe).ok_or_else(headless)?;
            Ok((body, "node"))
        }
    }
}

/// `code`'s blob in the cache, mapped, its body what derefs, and where it
/// came from: the cache's copy when it hashes to `code` (`disk`), else the
/// node's, streamed into the cache first ([`fetched`]; `node`). The outer
/// error is the cache's (it could not keep the blob: a full disk), the
/// inner one the node's or the blob's.
async fn cached(
    client: &RpcClient,
    code: &BlobId,
) -> std::io::Result<Result<(Blob, &'static str), Fetch>> {
    let path = cache_path(code);
    let kept = {
        let (path, code) = (path.clone(), *code);
        blocking(move || match Blob::mapped(&path) {
            Ok(blob) => Ok(hashes_to(&blob, &code)?.then_some(blob)),
            Err(_) => Ok(None),
        })
        .await
    };
    match kept {
        Ok(Some(blob)) => {
            return Ok(blob
                .part(unframe)
                .map(|body| (body, "disk"))
                .ok_or_else(headless));
        }
        Ok(None) => {}
        Err(error) => return Ok(Err(error)),
    }
    if let Err(error) = fetched(client, code, &path).await? {
        return Ok(Err(error));
    }
    // hashed as it landed: mapped as it is
    let landed = blocking(move || {
        Blob::mapped(&path)
            .map_err(|error| Fetch::Unreadable(format!("the program's code was not kept: {error}")))
    })
    .await;
    Ok(landed
        .and_then(|blob| blob.part(unframe).ok_or_else(headless))
        .map(|body| (body, "node")))
}

/// A blob no listed program runs (an upload, a scheduled change's code),
/// its body what derefs, read as [`program_bytes`] reads code but kept
/// nowhere: streamed into a file no path names, which the system removes
/// once the map of it goes, so neither the cache nor the heap holds it.
pub async fn unkept_bytes(client: &RpcClient, code: &BlobId) -> Result<Blob, Fetch> {
    let unkept =
        |error: std::io::Error| Fetch::Unreadable(format!("the blob was not read: {error}"));
    let file = blocking(move || tempfile::tempfile().map_err(unkept)).await?;
    let file = std::sync::Arc::new(file);
    streamed(client, code, file.clone())
        .await
        .map_err(unkept)??;
    blocking(move || Blob::unnamed(&file).map_err(unkept))
        .await?
        .part(unframe)
        .ok_or_else(headless)
}

/// `work` on tokio's blocking pool: a map's pages are read from the disk
/// as they are first touched.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Fetch> + Send + 'static,
) -> Result<T, Fetch> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Fetch::Unreadable(error.to_string()))?
}

fn headless() -> Fetch {
    Fetch::Unreadable("the blob carries no git header".into())
}

/// `code`'s blob streamed off the node into the cache at `path`, hashed as
/// it lands, never held whole: renamed into place once all of it came and
/// it hashes to `code`. A short or bad stream leaves no file. The outer
/// error is the cache's (it could not keep the blob: a full disk), the
/// inner one the node's. The landing file is named by this process too,
/// so another instance of the app landing the same code never truncates
/// it. Each piece goes to the disk as the connection read it, never
/// copied: a copy (tokio's file takes one of each write, up to 2 MB) is a
/// second piece-sized block on the reading thread, which glibc keeps in
/// that thread's arena once freed.
async fn fetched(
    client: &RpcClient,
    code: &BlobId,
    path: &Path,
) -> std::io::Result<Result<(), Fetch>> {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let landing = path.with_extension(format!("{}-{n}.partial", std::process::id()));
    let landed = async {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::File::create(&landing).await?.into_std().await;
        Ok(
            match streamed(client, code, std::sync::Arc::new(file)).await? {
                Ok(()) => Ok(tokio::fs::rename(&landing, path).await?),
                Err(error) => Err(error),
            },
        )
    }
    .await;
    if !matches!(landed, Ok(Ok(()))) {
        let _ = tokio::fs::remove_file(&landing).await;
    }
    landed
}

/// `code`'s blob streamed off the node into `file`, hashed as it lands,
/// never held whole: `Ok` once all of it came and it hashes to `code`. The
/// outer error is the file's, the inner one the node's or the blob's.
async fn streamed(
    client: &RpcClient,
    code: &BlobId,
    file: std::sync::Arc<std::fs::File>,
) -> std::io::Result<Result<(), Fetch>> {
    use std::io::Write as _;
    let mut blob = match client.blob_stream(*code).await {
        Ok(Some(blob)) => blob,
        Ok(None) => return Ok(Err(Fetch::NotHeld)),
        Err(error) => return Ok(Err(fetch_error(error))),
    };
    let mut hasher = blob::Hasher::new(code.kind());
    loop {
        let piece = match blob.next().await {
            Ok(Some(piece)) => piece,
            Ok(None) => break,
            Err(error) => return Ok(Err(fetch_error(error))),
        };
        hasher.update(&piece);
        let file = file.clone();
        tokio::task::spawn_blocking(move || (&*file).write_all(&piece))
            .await
            .map_err(std::io::Error::other)??;
    }
    Ok(match hasher.finish() {
        Ok(landed) if landed == *code => Ok(()),
        Ok(_) => Err(Fetch::Corrupt),
        Err(_) => Err(Fetch::Collision(*code)),
    })
}

/// Where the cache keeps `code`'s framed bytes.
pub(crate) fn cache_path(code: &BlobId) -> PathBuf {
    super::dirs()
        .cache
        .join("programs")
        .join(abi::hex(code.digest()))
}

/// A blob id names the FRAMED bytes (`kind len\0body`), as git's does, and
/// the cache keeps them framed: the kind is the node's word (a program is
/// `program`), which the id alone cannot give back.
fn hashes_to(framed: &[u8], code: &BlobId) -> Result<bool, Fetch> {
    let hashed = blob::id_of(code.kind(), framed).map_err(|_| Fetch::Collision(*code))?;
    Ok(hashed == *code)
}

/// The notifier inside `code`'s blob, or `None` when the program ships
/// none (a view-only entry never does). Every listed program's blob is
/// asked, to find the few that carry one, and a blob is megabytes, most of
/// it the view: it is mapped from the cache ([`cached`]) and the notifier
/// alone handed on, as a part of the map. A cache that cannot keep it (a
/// full disk) fails the read: a notifier is not worth a whole blob held.
pub async fn notifier_of(client: &RpcClient, code: &BlobId) -> Result<Option<Blob>, Fetch> {
    let (blob, _) = cached(client, code).await.map_err(|error| {
        Fetch::Unreadable(format!("the program's code could not be kept: {error}"))
    })??;
    Ok(blob.part(|body| section(body, NOTIFY_SECTION)))
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

    /// A section of a blob mapped from the cache is a part of the map,
    /// nothing copied: it reads as the section's bytes, and the code it
    /// hands a runner names the file and where in it the section starts,
    /// for a compile apart to read there. Bytes in memory name no file;
    /// a part read out of other bytes is none.
    #[test]
    fn a_section_of_a_mapped_blob_names_where_it_lies_in_the_file() {
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
        let framed = [format!("program {}\0", body.len()).as_bytes(), &body].concat();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        std::fs::write(&path, &framed).unwrap();
        let notifier = Blob::mapped(&path)
            .unwrap()
            .part(unframe)
            .unwrap()
            .part(|body| section(body, NOTIFY_SECTION))
            .unwrap();
        assert_eq!(&*notifier, b"the notifier");
        let at = (framed.len() - b"the notifier".len()) as u64;
        let code = notifier.code();
        assert_eq!(code.bytes, b"the notifier");
        assert_eq!(code.file, Some((path.as_path(), at)));
        let read = Blob::from(framed.clone()).part(unframe).unwrap();
        assert_eq!(&*read, &body[..]);
        assert_eq!(read.code().file, None);
        assert!(read.part(|_| Some(&b"elsewhere"[..])).is_none());
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
        assert_eq!(super::section(&module, VIEW_SECTION), Some(&body[..]));
        assert_eq!(super::section(b"\0asm\x01\0\0\0", VIEW_SECTION), None);
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
