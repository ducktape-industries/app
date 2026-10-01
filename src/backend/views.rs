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
/// request takes.
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
    Ok(programs.chain(views).collect())
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
    fetched(client, code, cache_path(code)).await
}

/// `code`'s body: out of the cache file at `cached` while its bytes hash to
/// `code`, else from the node, and kept at `cached` for the next load.
async fn fetched(
    client: &RpcClient,
    code: &BlobId,
    cached: Option<PathBuf>,
) -> Result<(Vec<u8>, &'static str), Fetch> {
    if let Some(path) = &cached
        && let Ok(framed) = tokio::fs::read(path).await
        && hashes_to(&framed, code)
        && let Some(body) = super::noded::unframe(&framed)
    {
        return Ok((body.to_vec(), "disk"));
    }
    let framed = client
        .blob(*code)
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
        let _ = tokio::fs::write(path, framed).await;
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
/// the kind is the store's word (code is framed `program`), not the app's:
/// the cache keeps the framed bytes, so a cached blob is checked exactly as
/// a fetched one is.
fn hashes_to(framed: &[u8], code: &BlobId) -> bool {
    use sha2::Digest as _;
    match code.kind() {
        abi::HashKind::Sha1 => sha1::Sha1::digest(framed)[..] == *code.digest(),
        abi::HashKind::Sha256 => sha2::Sha256::digest(framed)[..] == *code.digest(),
    }
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

    /// `body` framed as the node's store frames code (`program len\0body`),
    /// and the id that names it.
    fn program(body: &[u8]) -> (Vec<u8>, BlobId) {
        use sha2::Digest as _;
        let framed = [format!("program {}\0", body.len()).as_bytes(), body].concat();
        let code = BlobId::Sha256(sha2::Sha256::digest(&framed).into());
        (framed, code)
    }

    /// A node that answers every blob request with `framed`, and counts them.
    async fn node_holding(
        framed: Vec<u8>,
    ) -> (RpcClient, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::Ordering;
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let client = RpcClient::new(format!("http://{}", listener.local_addr().unwrap()));
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked.clone();
        let answer = abi::encode(&Some(framed));
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (_, path, _) = crate::backend::loopback::read_request(&mut stream)
                    .await
                    .unwrap();
                assert_eq!(path, crate::backend::noded::route::BLOB_GET);
                counted.fetch_add(1, Ordering::SeqCst);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    answer.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(&answer).await.unwrap();
            }
        });
        (client, asked)
    }

    #[tokio::test]
    async fn a_program_blob_is_served_from_disk_on_the_second_load() {
        use std::sync::atomic::Ordering;
        // a wasm body opens with a NUL, exactly where a frame's header ends
        let body = b"\0asm\x01\0\0\0".to_vec();
        let (framed, code) = program(&body);
        let (client, asked) = node_holding(framed).await;
        let dir = tempfile::tempdir().unwrap();
        let cached = Some(dir.path().join("programs").join("code"));
        let first = fetched(&client, &code, cached.clone()).await;
        assert_eq!(first, Ok((body.clone(), "node")));
        let second = fetched(&client, &code, cached).await;
        assert_eq!(second, Ok((body, "disk")));
        assert_eq!(asked.load(Ordering::SeqCst), 1, "the node is asked once");
    }

    #[tokio::test]
    async fn a_cached_file_that_does_not_hash_to_the_id_is_refused_and_fetched_again() {
        use std::sync::atomic::Ordering;
        let body = b"\0asm\x01\0\0\0".to_vec();
        let (framed, code) = program(&body);
        let mut corrupt = framed.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        // the body alone is what the cache kept before it kept the frame,
        // `blob` the kind it then guessed
        let reframed = [&b"blob 8\0"[..], &body[..]].concat();
        let (client, asked) = node_holding(framed.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("code");
        for (nth, stale) in [corrupt, body.clone(), reframed].into_iter().enumerate() {
            std::fs::write(&path, stale).unwrap();
            let refetched = fetched(&client, &code, Some(path.clone())).await;
            assert_eq!(refetched, Ok((body.clone(), "node")));
            assert_eq!(asked.load(Ordering::SeqCst), nth + 1);
            // the refetch replaced the file
            assert_eq!(std::fs::read(&path).unwrap(), framed);
        }
        let next = fetched(&client, &code, Some(path)).await;
        assert_eq!(next, Ok((body, "disk")));
        assert_eq!(asked.load(Ordering::SeqCst), 3, "the disk asks no node");
    }
}
