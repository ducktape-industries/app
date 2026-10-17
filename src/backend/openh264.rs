//! Cisco's OpenH264 binary, the H.264 of a call on Linux
//! (`ducktape_av::codec::cisco`). Cisco's AVC/H.264 patent licence covers
//! only the build Cisco made, fetched by the user's own machine: never
//! bundled, so it comes from ciscobinary.openh264.org the first time a
//! call needs a picture, into the data dir, its sha256 checked against
//! the build this app knows before anything is written, its licence text
//! written beside it. Settings › About switches it off and on (`openh264`
//! in prefs.json; on until turned off), shows Cisco's line and where the
//! licence is. Off, not fetched, or offline: a call goes on by voice.
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::Digest as _;

const VERSION: &str = "2.6.0";
/// Cisco's file for this machine and its sha256, as the `openh264` crate's
/// table of known builds has them (it checks again when loading).
#[cfg(target_arch = "x86_64")]
const BUILD: (&str, &str) = (
    "libopenh264-2.6.0-linux64.8.so",
    "2f0cde7c6a6abcf5cae76942894ea42897fa677bce4ed6c91a24dd1b041d5f04",
);
#[cfg(target_arch = "aarch64")]
const BUILD: (&str, &str) = (
    "libopenh264-2.6.0-linux-arm64.8.so",
    "12e7b33623667cdab0e575170c147b1b36eadb77d0d2aa7ceb5afd3e58902140",
);
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const BUILD: (&str, &str) = ("", "");

/// What Cisco asks a product using its binary to show.
pub(crate) const ATTRIBUTION: &str = "OpenH264 Video Codec provided by Cisco Systems, Inc.";
const LICENSE: &str = include_str!("openh264-license.txt");

/// Tells the codecs where the library is, once at start.
pub(crate) fn supply() {
    ducktape_av::codec::cisco::supply(library);
}

/// Whether the binary may be fetched and used: on until turned off.
pub(crate) fn enabled() -> bool {
    super::read_prefs().unwrap_or_default()["openh264"]
        .as_bool()
        .unwrap_or(true)
}

pub(crate) fn set_enabled(on: bool) {
    super::edit_prefs(|prefs| prefs["openh264"] = serde_json::json!(on));
}

/// Where the licence text is saved, beside the binary (once fetched).
pub(crate) fn license_path() -> String {
    super::data_dir()
        .map(|dir| {
            dir.join("openh264")
                .join("LICENSE.txt")
                .display()
                .to_string()
        })
        .unwrap_or_default()
}

/// How long a fetch that failed is not tried again: a ladder step or a
/// new encoder in the same call does not knock on Cisco's door each time.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// One fetch at a time (a call's publishing and playing threads ask
/// together), and the last failure, held back for a while.
#[derive(Default)]
struct Fetching {
    failed: Option<(Instant, String)>,
}

static FETCHING: Mutex<Fetching> = Mutex::new(Fetching { failed: None });

/// Where the library is, fetched now if it is not yet; why not, when it is
/// turned off or cannot be fetched. Called on a call's own thread, the
/// first time a picture is to be sent or shown.
fn library() -> Result<PathBuf, String> {
    if !enabled() {
        return Err("Cisco's H.264 is turned off in Settings".into());
    }
    let (name, sha256) = BUILD;
    if name.is_empty() {
        return Err(format!("no Cisco build for {}", std::env::consts::ARCH));
    }
    let dir = super::data_dir()?.join("openh264");
    let path = fetched(&FETCHING, &dir, name, || {
        let packed = fetch(&format!("https://ciscobinary.openh264.org/{name}.bz2"))?;
        unpack(&packed, sha256)
    })?;
    Ok(path)
}

/// `dir/name`, fetched by `fetch` and written (its licence beside it)
/// when it is not there yet: once, however many threads ask at once; a
/// failure is answered again, without fetching, for [`RETRY_AFTER`].
fn fetched(
    fetching: &Mutex<Fetching>,
    dir: &Path,
    name: &str,
    fetch: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<PathBuf, String> {
    let path = dir.join(name);
    if path.exists() {
        return Ok(path);
    }
    let mut fetching = fetching
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // another thread fetched it while this one waited
    if path.exists() {
        return Ok(path);
    }
    if let Some((when, error)) = &fetching.failed
        && when.elapsed() < RETRY_AFTER
    {
        return Err(error.clone());
    }
    let written = fetch().and_then(|library| {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let part = dir.join(format!("{name}.part"));
        std::fs::write(&part, library)
            .and_then(|()| std::fs::write(dir.join("LICENSE.txt"), LICENSE))
            .and_then(|()| std::fs::rename(&part, &path))
            .map_err(|e| format!("{}: {e}", path.display()))
    });
    match written {
        Ok(()) => {
            fetching.failed = None;
            tracing::info!(target: "ducktape::call", path = %path.display(), version = VERSION, "Cisco's OpenH264 fetched");
            Ok(path)
        }
        Err(error) => {
            tracing::warn!(target: "ducktape::call", %error, "Cisco's OpenH264 not fetched");
            fetching.failed = Some((Instant::now(), error.clone()));
            Err(error)
        }
    }
}

/// The whole of `url`, on a runtime of this thread's own (a call's
/// thread has none); 10 s to connect, 60 s for the whole.
fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| e.to_string())?
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| format!("{url}: {e}"))?
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| format!("{url}: {e}"))
    })
}

/// The library out of Cisco's bzip2 file, if it is the build known by
/// `sha256`.
fn unpack(packed: &[u8], sha256: &str) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let mut library = Vec::new();
    bzip2::read::BzDecoder::new(packed)
        .read_to_end(&mut library)
        .map_err(|e| format!("not a bzip2 file: {e}"))?;
    let digest = sha2::Sha256::digest(&library);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    match hex == sha256 {
        true => Ok(library),
        false => Err(format!("a file with sha256 {hex}, not Cisco's {sha256}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file that is not the known build is refused before it is written.
    #[test]
    fn only_ciscos_known_build_unpacks() {
        let mut packed = Vec::new();
        {
            use std::io::Write as _;
            let mut encoder = bzip2::write::BzEncoder::new(&mut packed, bzip2::Compression::fast());
            encoder.write_all(b"not a library").unwrap();
            encoder.finish().unwrap();
        }
        let refused = unpack(&packed, BUILD.1).unwrap_err();
        assert!(refused.contains("not Cisco's"), "{refused}");
        assert!(unpack(b"garbage", BUILD.1).unwrap_err().contains("bzip2"));
        let sha = {
            let digest = sha2::Sha256::digest(b"not a library");
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(unpack(&packed, &sha).unwrap(), b"not a library");
    }

    /// Two threads asking at once fetch once and both get the file; a
    /// fetch that failed is not tried again at once, and the file, once
    /// there, is answered without the lock's say.
    #[test]
    fn a_first_fetch_happens_once_for_every_thread_asking() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir =
            std::env::temp_dir().join(format!("ducktape-openh264-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fetching = Mutex::new(Fetching::default());
        let fetches = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    let path = fetched(&fetching, &dir, "lib.so", || {
                        fetches.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        Ok(b"library".to_vec())
                    })
                    .unwrap();
                    assert_eq!(std::fs::read(path).unwrap(), b"library");
                });
            }
        });
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert!(dir.join("LICENSE.txt").exists());

        let failing = Mutex::new(Fetching::default());
        let tries = AtomicUsize::new(0);
        let attempt = || {
            fetched(&failing, &dir, "missing.so", || {
                tries.fetch_add(1, Ordering::SeqCst);
                Err("offline".to_string())
            })
        };
        assert_eq!(attempt().unwrap_err(), "offline");
        assert_eq!(attempt().unwrap_err(), "offline");
        assert_eq!(tries.load(Ordering::SeqCst), 1, "a failure is held back");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
