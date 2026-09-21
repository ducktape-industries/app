//! Where a view comes from: the connected node's roster. `/v1/programs`
//! names every program and the code blob it runs; a program that ships a
//! view carries it inside that blob, as the custom section
//! [`VIEW_SECTION`] — one artifact, one blob id. The app reads the section out and mounts it. A program without the section
//! has no view, which is the network's fact, not a failure.
//!
//! Nothing here knows a program by name: the roster's order is the rail's
//! order, the manifest inside the view names the tab.
//!
//! ponytail: the view rides the program blob, so a view-only change
//! re-publishes the program. A view blob id beside the entry would split
//! them; that needs the roster contract (sdk `abi::roster`) to grow a field.

use std::path::PathBuf;

use abi::BlobId;

use super::{RpcClient, cache_dir};

pub const VIEW_SECTION: &str = "ducktape.view";

/// One roster entry, as the rail lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub name: String,
    pub code: BlobId,
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

/// The roster, in the node's (name) order.
pub async fn programs(client: &RpcClient) -> Result<Vec<Program>, Fetch> {
    let listed = client.programs().await.map_err(unreachable)?;
    Ok(listed
        .into_iter()
        .map(|(name, code)| Program { name, code })
        .collect())
}

/// The view inside `code`'s blob, or `None` when the program ships none.
/// A blob read once is kept in the cache directory under its id.
pub async fn view_of(client: &RpcClient, code: &BlobId) -> Result<Option<Vec<u8>>, Fetch> {
    let program = program_bytes(client, code).await?;
    Ok(view_section(&program))
}

async fn program_bytes(client: &RpcClient, code: &BlobId) -> Result<Vec<u8>, Fetch> {
    let cached = cache_path(code);
    if let Some(path) = &cached
        && let Ok(bytes) = tokio::fs::read(path).await
        && hashes_to(&bytes, code)
    {
        return Ok(bytes);
    }
    let framed = client
        .blob(*code)
        .await
        .map_err(unreachable)?
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
    Ok(body)
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
    let framed = match super::noded::unframe(bytes) {
        Some(_) => bytes.to_vec(),
        None => {
            let mut framed = format!("blob {}\0", bytes.len()).into_bytes();
            framed.extend_from_slice(bytes);
            framed
        }
    };
    match code.kind() {
        abi::HashKind::Sha1 => sha1::Sha1::digest(&framed)[..] == *code.digest(),
        abi::HashKind::Sha256 => sha2::Sha256::digest(&framed)[..] == *code.digest(),
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

fn unreachable(error: super::noded::Error) -> Fetch {
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

    #[test]
    fn a_cached_body_hashes_like_the_framed_blob() {
        use sha2::Digest as _;
        let body = b"hello";
        let framed = b"blob 5\0hello";
        let id = BlobId::Sha256(sha2::Sha256::digest(framed).into());
        assert!(hashes_to(body, &id));
        assert!(hashes_to(framed, &id));
        assert!(!hashes_to(b"other", &id));
    }
}
