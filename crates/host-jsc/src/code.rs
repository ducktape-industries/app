//! The prepared code, apart from WebKit so it is tested everywhere: metered
//! code by the sha256 of its unmetered bytes, kept while anything holds it
//! (contract `Prepared::hold`), and what a load the page refused does to
//! it. One code per hash for every seat that holds it, so a seat's failure
//! never takes the code from another seat: its cached file goes at once,
//! and the next `prepare` meters it again in place.

use crate::lock;
use ducktape_host::contract::{CodeSource, Failure};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// Metered code the page loads by `name`, kept by whatever may still seat
/// it (contract `Prepared::hold`) and by every instance of it.
pub(crate) struct Code {
    pub(crate) name: String,
    view_bytes: u64,
    /// What the page is served as `name`.
    bytes: Mutex<Arc<[u8]>>,
    /// The page could not load `bytes` ([`Code::not_loaded`]).
    unloaded: AtomicBool,
}

impl Code {
    pub(crate) fn new(name: String, view_bytes: u64, bytes: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            name,
            view_bytes,
            bytes: Mutex::new(Arc::from(bytes)),
            unloaded: AtomicBool::new(false),
        })
    }

    /// What the page is served as `name`.
    pub(crate) fn bytes(&self) -> Arc<[u8]> {
        lock(&self.bytes).clone()
    }

    /// The page could not load this code: its file in `cache` goes now (a
    /// corrupt one would fail the view on every launch), and the next
    /// `prepare` meters it again, in place. Every seat that holds it keeps
    /// it meanwhile and seats it by hash as before: one seat's failure is
    /// not another's.
    pub(crate) fn not_loaded(&self, cache: Option<&Path>) {
        self.unloaded.store(true, Ordering::Release);
        if let Some(cache) = cache {
            // one that cannot be removed is metered again only once it is
            // purged; nothing else depends on it
            let _gone = std::fs::remove_file(cache.join(&self.name));
        }
    }
}

/// Prepared code by the sha256 of its unmetered bytes, the last one
/// prepared for it, while anything holds it.
#[derive(Default)]
pub(crate) struct Codes(Mutex<HashMap<[u8; 32], Weak<Code>>>);

impl Codes {
    /// `code` (sha256 `hash`) prepared for `view_bytes`: the code held for
    /// it, as it is, or metered again in place when the page could not load
    /// it; else metered now ([`crate::metered`], kept under `cache`) and
    /// kept while anything holds it.
    pub(crate) fn prepare(
        &self,
        cache: Option<&Path>,
        hash: &[u8; 32],
        code: &[u8],
        view_bytes: u64,
    ) -> Result<(Arc<Code>, CodeSource), Failure> {
        if let Some(held) = self.held(hash).filter(|held| held.view_bytes == view_bytes) {
            if !held.unloaded.load(Ordering::Acquire) {
                return Ok((held, CodeSource::Memory));
            }
            let (bytes, source) = crate::metered(cache, hash, code, view_bytes)?;
            *lock(&held.bytes) = Arc::from(bytes);
            held.unloaded.store(false, Ordering::Release);
            return Ok((held, source));
        }
        let (bytes, source) = crate::metered(cache, hash, code, view_bytes)?;
        let prepared = Code::new(crate::file_name(hash, view_bytes), view_bytes, bytes);
        let mut kept = lock(&self.0);
        kept.retain(|_, code| code.strong_count() > 0);
        kept.insert(*hash, Arc::downgrade(&prepared));
        Ok((prepared, source))
    }

    /// The code prepared for `hash`, while anything holds it.
    pub(crate) fn held(&self, hash: &[u8; 32]) -> Option<Arc<Code>> {
        lock(&self.0).get(hash).and_then(Weak::upgrade)
    }

    /// The held code the page loads as `name`.
    pub(crate) fn named(&self, name: &str) -> Option<Arc<Code>> {
        lock(&self.0)
            .values()
            .filter_map(Weak::upgrade)
            .find(|code| code.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) i32.const 64)
        (func (export "init"))
        (func (export "tick") (param i32 i32) (result i64) i64.const 0)
        (func (export "snapshot") (result i64) i64.const 0)
        (func (export "restore") (param i32 i32) (result i64) i64.const 0))"#;

    /// A corrupt cached file the page could not load, while two seats hold
    /// its code: the file goes at once, the other seat still seats the code
    /// by hash, and the failing seat's next `prepare` meters it again in
    /// place, so both seat the good code from then on.
    #[test]
    fn code_the_page_could_not_load_stays_for_every_seat_and_is_metered_again() {
        let dir = std::env::temp_dir().join(format!("host-jsc-code-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (hash, budget) = ([7; 32], 4 << 16);
        let file = dir.join(crate::file_name(&hash, budget));
        std::fs::write(&file, b"not wasm").unwrap();
        let code = wat::parse_str(VIEW).unwrap();
        let codes = Codes::default();

        let (failing, source) = codes.prepare(Some(&dir), &hash, &code, budget).unwrap();
        assert_eq!(source, CodeSource::Disk);
        let (other, source) = codes.prepare(Some(&dir), &hash, &code, budget).unwrap();
        assert_eq!(source, CodeSource::Memory);
        assert!(Arc::ptr_eq(&failing, &other));

        failing.not_loaded(Some(&dir));
        drop(failing);
        assert!(!file.exists(), "the corrupt file was kept");
        let seated = codes.held(&hash).expect("the other seat lost its code");
        assert!(Arc::ptr_eq(&seated, &other));
        assert!(
            codes.named(&other.name).is_some(),
            "the page is not served it"
        );

        let (again, source) = codes.prepare(Some(&dir), &hash, &code, budget).unwrap();
        assert_eq!(
            source,
            CodeSource::Cold,
            "the failing seat got the bytes back"
        );
        assert!(Arc::ptr_eq(&again, &other), "two codes for one hash");
        let metered = view_meter::meter(&code, budget).unwrap();
        assert_eq!(&*other.bytes(), &metered[..]);
        assert_eq!(
            std::fs::read(&file).unwrap(),
            metered,
            "the good code is not kept"
        );
        let (_, source) = codes.prepare(Some(&dir), &hash, &code, budget).unwrap();
        assert_eq!(source, CodeSource::Memory);

        // code nothing holds any more is let go
        drop((seated, other, again));
        assert!(codes.held(&hash).is_none());
        let _gone = std::fs::remove_dir_all(&dir);
    }
}
