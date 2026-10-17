//! The update executor against a local bucket: a plain-HTTP server on
//! loopback stands in for `dl.ducktape.industries`, a throwaway key for the
//! release key, and shell scripts that answer `--version` for releases. The
//! boot step is driven through [`boot_at`] with the "running executable"
//! named explicitly, so a test plays every start of the app in turn.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ducktape_updater::layout::Channel;
use ducktape_updater::release::signing_key_from_hex;
use ducktape_updater::{
    Artifact, Manifest, Platform, PublicKey, Refusal as ManifestRefusal, Release, SCHEMA, Sha,
    Signature, SignedManifest, Take, TrustedKeys, layout,
};

use super::channel::{client, download, fetch};
use super::stage::tests::{Item, archive_of};
use super::{APP_EXE, Layout};

/// The test release key's seed.
fn seed() -> String {
    "29".repeat(32)
}

fn test_key() -> PublicKey {
    PublicKey::of(&signing_key_from_hex(&seed()).unwrap())
}

fn keys() -> TrustedKeys {
    TrustedKeys {
        pinned: test_key(),
        successor: None,
    }
}

/// A channel's file: `manifest` and the test key's signature over it.
fn channel_file(manifest: &Manifest) -> Vec<u8> {
    let manifest_bytes = serde_json::to_vec_pretty(manifest).unwrap();
    let signature = Signature::sign(&signing_key_from_hex(&seed()).unwrap(), &manifest_bytes);
    SignedManifest {
        manifest_bytes,
        signature,
    }
    .file()
    .into_bytes()
}

fn block<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

// ---- the bucket ------------------------------------------------------------

/// Every request a bucket heard: the file, and the `Range` start it asked.
type Asked = Arc<Mutex<Vec<(String, Option<u64>)>>>;
/// What runs as a file is asked for, before it is answered: once each.
type Hooks = Arc<Mutex<Vec<(String, Box<dyn FnOnce() + Send>)>>>;

/// A bucket on loopback: files by name, every request recorded.
struct Bucket {
    base: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    asked: Asked,
    /// Answer every request whole, as a server without range support does.
    ignore_range: Arc<AtomicBool>,
    hooks: Hooks,
}

impl Bucket {
    fn new() -> Bucket {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let bucket = Bucket {
            base,
            files: Arc::default(),
            asked: Arc::default(),
            ignore_range: Arc::default(),
            hooks: Arc::default(),
        };
        let (files, asked, ignore_range, hooks) = (
            bucket.files.clone(),
            bucket.asked.clone(),
            bucket.ignore_range.clone(),
            bucket.hooks.clone(),
        );
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &files, &asked, &ignore_range, &hooks);
            }
        });
        bucket
    }

    /// Run `then` as `name` is first asked for, before it is answered.
    #[cfg(target_os = "linux")]
    fn when_asked(&self, name: &str, then: impl FnOnce() + Send + 'static) {
        self.hooks
            .lock()
            .unwrap()
            .push((name.to_string(), Box::new(then)));
    }

    fn put(&self, name: &str, bytes: Vec<u8>) {
        self.files.lock().unwrap().insert(name.to_string(), bytes);
    }

    fn asked(&self) -> Vec<(String, Option<u64>)> {
        self.asked.lock().unwrap().clone()
    }

    /// How many times `name` was asked for.
    fn asked_for(&self, name: &str) -> usize {
        self.asked().iter().filter(|(path, _)| path == name).count()
    }
}

fn serve(
    mut stream: TcpStream,
    files: &Mutex<HashMap<String, Vec<u8>>>,
    asked: &Mutex<Vec<(String, Option<u64>)>>,
    ignore_range: &AtomicBool,
    hooks: &Hooks,
) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head).to_string();
    let path = head
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_string();
    let range = head.lines().find_map(|line| {
        let line = line.to_ascii_lowercase();
        let from = line
            .strip_prefix("range: bytes=")?
            .trim_end_matches('-')
            .to_string();
        from.parse::<u64>().ok()
    });
    asked.lock().unwrap().push((path.clone(), range));
    let due: Vec<_> = {
        let mut hooks = hooks.lock().unwrap();
        let (due, kept) = std::mem::take(&mut *hooks)
            .into_iter()
            .partition(|(name, _)| *name == path);
        *hooks = kept;
        due
    };
    for (_, then) in due {
        then();
    }
    let body = files.lock().unwrap().get(&path).cloned();
    let range = range.filter(|_| !ignore_range.load(Ordering::SeqCst));
    let (status, extra, body) = match (body, range) {
        (None, _) => ("404 Not Found", String::new(), Vec::new()),
        (Some(body), Some(from)) => {
            let total = body.len();
            let from = (from as usize).min(total);
            (
                "206 Partial Content",
                format!(
                    "Content-Range: bytes {from}-{}/{total}\r\n",
                    total.saturating_sub(1)
                ),
                body[from..].to_vec(),
            )
        }
        (Some(body), None) => ("200 OK", String::new(), body),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(&body);
}

/// `release` published on `channel` at `sequence`: its archive and the pair
/// signed by the test key. The archive's sha.
fn publish(bucket: &Bucket, channel: Channel, sequence: u64, release: &[Item]) -> Sha {
    let archive = archive_of(release);
    let sha = Sha::digest(&archive);
    let manifest = Manifest {
        schema: SCHEMA,
        channel: channel.name().into(),
        sequence,
        published_at: "2026-10-13T00:00:00Z".into(),
        release: Release {
            sha256_id: Sha::ZERO,
            display: format!("0.1.0+{}", sha.short()),
            notes_url: String::new(),
        },
        artifacts: BTreeMap::from([(
            Platform::HOST.key(),
            Artifact {
                sha256: sha,
                size: archive.len() as u64,
            },
        )]),
        successor_key: None,
    }
    .sealed();
    bucket.put(&layout::archive_name(&sha, &Platform::HOST.key()), archive);
    bucket.put(&channel.file(), channel_file(&manifest));
    sha
}

/// A scratch install: every root under one temp dir.
fn rig() -> (tempfile::TempDir, Layout) {
    let root = tempfile::tempdir().unwrap();
    let layout = Layout {
        updates: root.path().join("cfg/ducktape/updates"),
        install: root.path().join("data/ducktape"),
        data_home: root.path().join("data"),
    };
    (root, layout)
}

/// A release's executable: answers `--version` as the app does, and does
/// nothing else.
fn script(name: &str) -> Vec<u8> {
    format!(
        "#!/bin/sh\ncase \"$1\" in --version) echo \"ducktape-app 0.0.0+{name}\";; esac\nexit 0\n"
    )
    .into_bytes()
}

/// A release tree holding `exe` as the app.
fn release_of(exe: Vec<u8>) -> Vec<Item> {
    vec![Item::File(APP_EXE, exe, 0o755)]
}

// ---- the restart -----------------------------------------------------------

/// The restart's waiter hands the install over free: it waits while another
/// process holds `<updates>/lock` (the app that pressed Restart, as the
/// kernel closes its files), and goes on once it is let go, holding nothing.
#[test]
fn the_restart_waiter_waits_until_the_install_is_let_go() {
    use std::os::unix::io::AsRawFd as _;
    use std::time::Duration;
    let (_root, layout) = rig();
    std::fs::create_dir_all(&layout.updates).unwrap();
    let old = std::fs::File::create(layout.updates.join("lock")).unwrap();
    // SAFETY: an open descriptor and a flag constant.
    assert_eq!(unsafe { libc::flock(old.as_raw_fd(), libc::LOCK_EX) }, 0);
    let (done, waited) = std::sync::mpsc::channel();
    let waiting = layout.clone();
    std::thread::spawn(move || done.send(super::await_install(&waiting).is_ok()).unwrap());
    assert!(
        waited.recv_timeout(Duration::from_millis(300)).is_err(),
        "it waits while the old process holds the install"
    );
    drop(old);
    assert!(waited.recv_timeout(Duration::from_secs(10)).unwrap());
    assert!(
        super::hold_install(&layout).unwrap(),
        "the next start takes it"
    );
}

/// The macOS boot step lets the install go before it starts the new launch,
/// which must take it: a start that finds it held runs with the updater off.
#[test]
fn a_new_launch_finds_the_install_let_go() {
    use std::os::unix::io::AsRawFd as _;
    let (_root, layout) = rig();
    assert!(super::hold_install(&layout).unwrap());
    let next = std::fs::File::open(layout.updates.join("lock")).unwrap();
    // SAFETY: an open descriptor and flag constants.
    let take = || unsafe { libc::flock(next.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_ne!(take(), 0, "held by the process that flipped");
    super::let_go_install(&layout);
    assert_eq!(take(), 0, "the new launch takes it");
}

// ---- the channel: fetch and download ----------------------------------------

/// Each channel is one file, its manifest and the signature over it, read
/// whole in one request from the channel's own name, and verifies under the
/// pinned key; a stranger's signature over the same manifest is refused,
/// and a channel with nothing published is a check that did not happen,
/// not a refusal.
#[test]
fn a_channel_file_verifies_and_a_strangers_signature_is_refused() {
    let bucket = Bucket::new();
    let client = client();
    let sequence = |channel| {
        let fetched = block(fetch(&client, &bucket.base, channel, &keys()));
        fetched.map(|verified| verified.map(|verified| verified.manifest.sequence))
    };
    publish(&bucket, Channel::Stable, 7, &release_of(script("a")));
    assert_eq!(sequence(Channel::Nightly), None);
    publish(&bucket, Channel::Nightly, 9, &release_of(script("n")));
    assert_eq!(sequence(Channel::Stable), Some(Ok(7)));
    assert_eq!(sequence(Channel::Nightly), Some(Ok(9)));
    let asked: Vec<String> = bucket.asked().into_iter().map(|(path, _)| path).collect();
    assert_eq!(
        asked,
        ["nightly.json", "stable.json", "nightly.json"],
        "one file per check"
    );
    let signed = SignedManifest::from_file(&bucket.files.lock().unwrap()["stable.json"]).unwrap();
    let stranger = signing_key_from_hex(&"07".repeat(32)).unwrap();
    let forged = SignedManifest {
        signature: Signature::sign(&stranger, &signed.manifest_bytes),
        ..signed
    };
    bucket.put("stable.json", forged.file().into_bytes());
    assert_eq!(
        sequence(Channel::Stable),
        Some(Err(ManifestRefusal::BadSignature))
    );
}

/// A pick kept in `<updates>/channel` reads back as picked; no file, or
/// anything else in it, reads as stable.
#[test]
fn the_kept_channel_reads_back_and_anything_else_is_stable() {
    use super::channel::{keep_channel, read_channel};
    let (_root, layout) = rig();
    assert_eq!(read_channel(&layout), Channel::Stable);
    keep_channel(&layout, Channel::Nightly).unwrap();
    assert_eq!(read_channel(&layout), Channel::Nightly);
    keep_channel(&layout, Channel::Stable).unwrap();
    assert_eq!(read_channel(&layout), Channel::Stable);
    std::fs::write(layout.updates.join("channel"), "beta\n").unwrap();
    assert_eq!(read_channel(&layout), Channel::Stable);
}

/// "Update automatically" kept in `<updates>/automatic` reads back as set;
/// no file, or anything but `off` in it, reads as on.
#[test]
fn the_automatic_switch_reads_back_and_is_on_unless_it_says_off() {
    use super::channel::{keep_automatic, read_automatic};
    let (_root, layout) = rig();
    assert!(read_automatic(&layout), "no file is on");
    keep_automatic(&layout, false).unwrap();
    assert!(!read_automatic(&layout));
    keep_automatic(&layout, true).unwrap();
    assert!(read_automatic(&layout));
    std::fs::write(layout.updates.join("automatic"), "maybe\n").unwrap();
    assert!(read_automatic(&layout));
}

/// A large channel file is read whole and judged by what it holds.
#[test]
fn a_large_manifest_is_read_whole() {
    let bucket = Bucket::new();
    bucket.put("stable.json", vec![b' '; 300 * 1024]);
    assert!(matches!(
        block(fetch(&client(), &bucket.base, Channel::Stable, &keys())),
        Some(Err(_))
    ));
}

/// An archive of `size` bytes no two of which repeat in a short window.
fn archive_bytes(size: usize) -> Vec<u8> {
    (0..size).map(|at| (at * 7 % 251) as u8).collect()
}

/// An interrupted download resumes where the partial ends, with a `Range`,
/// and the whole file hashes to the artifact's sha.
#[test]
fn a_download_resumes_from_the_partial_with_a_range() {
    let bucket = Bucket::new();
    let (_root, layout) = rig();
    let archive = archive_bytes(100_000);
    let sha = Sha::digest(&archive);
    let name = layout::archive_name(&sha, &Platform::HOST.key());
    bucket.put(&name, archive.clone());
    let partial = layout.partial_path(sha);
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    std::fs::write(&partial, &archive[..4000]).unwrap();
    let size = archive.len() as u64;
    block(download(&client(), &bucket.base, &layout, sha, size)).unwrap();
    assert_eq!(bucket.asked(), vec![(name, Some(4000))]);
    assert_eq!(std::fs::read(&partial).unwrap(), archive);
}

/// A server that answers a range with the whole file: the partial starts
/// over rather than growing the whole file onto its own head.
#[test]
fn a_server_that_ignores_the_range_starts_the_file_over() {
    let bucket = Bucket::new();
    bucket.ignore_range.store(true, Ordering::SeqCst);
    let (_root, layout) = rig();
    let archive = archive_bytes(50_000);
    let sha = Sha::digest(&archive);
    bucket.put(
        &layout::archive_name(&sha, &Platform::HOST.key()),
        archive.clone(),
    );
    let partial = layout.partial_path(sha);
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    std::fs::write(&partial, &archive[..1000]).unwrap();
    let size = archive.len() as u64;
    block(download(&client(), &bucket.base, &layout, sha, size)).unwrap();
    assert_eq!(std::fs::read(&partial).unwrap(), archive);
}

/// Bytes that are not the artifact's are deleted, not resumed from.
#[test]
fn bytes_that_do_not_hash_to_the_sha_are_deleted() {
    let bucket = Bucket::new();
    let (_root, layout) = rig();
    let archive = archive_bytes(20_000);
    let sha = Sha::digest(&archive);
    let mut wrong = archive.clone();
    wrong[10] ^= 1;
    bucket.put(&layout::archive_name(&sha, &Platform::HOST.key()), wrong);
    let size = archive.len() as u64;
    let failed = block(download(&client(), &bucket.base, &layout, sha, size));
    assert_eq!(failed, Err("sha256_mismatch".into()));
    assert!(!layout.partial_path(sha).exists());
}

/// A partial longer than the artifact cannot be it: dropped, and nothing is
/// asked of the bucket.
#[test]
fn a_partial_longer_than_the_artifact_is_dropped_unasked() {
    let bucket = Bucket::new();
    let (_root, layout) = rig();
    let sha = Sha::digest(b"x");
    let partial = layout.partial_path(sha);
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    std::fs::write(&partial, [0u8; 64]).unwrap();
    let failed = block(download(&client(), &bucket.base, &layout, sha, 10));
    assert_eq!(failed, Err("partial_overlong".into()));
    assert!(!partial.exists());
    assert!(bucket.asked().is_empty());
}

/// The boot step runs before any window: a staged release that never
/// answers its self-check is refused once its time is up, and one that
/// answers but leaves a process holding its output is not waited on.
#[test]
fn a_self_check_is_never_waited_on_past_its_time() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, Instant};

    use super::boot::answers_version;
    let root = tempfile::tempdir().unwrap();
    let release = |name: &str, body: &str| {
        let exe = root.path().join(name);
        std::fs::write(&exe, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe
    };
    let hangs = release("hangs", "exec sleep 30");
    let lingers = release("lingers", "(sleep 30 &)\necho 'ducktape-app 0.0.0+l'");
    let started = Instant::now();
    let timeout = Duration::from_millis(500);
    assert_eq!(
        answers_version(&hangs, timeout),
        Err("qualify_timeout".into())
    );
    assert_eq!(answers_version(&lingers, Duration::from_secs(10)), Ok(()));
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// A `.partial` that is a link is not written through.
#[test]
fn a_partial_that_is_a_link_is_not_followed() {
    let bucket = Bucket::new();
    let (root, layout) = rig();
    let archive = archive_bytes(1000);
    let sha = Sha::digest(&archive);
    bucket.put(&layout::archive_name(&sha, &Platform::HOST.key()), archive);
    let elsewhere = root.path().join("elsewhere");
    std::fs::write(&elsewhere, b"keep").unwrap();
    let partial = layout.partial_path(sha);
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &partial).unwrap();
    let failed = block(download(&client(), &bucket.base, &layout, sha, 1000));
    assert_eq!(failed, Err("symlink_refused".into()));
    assert_eq!(std::fs::read(&elsewhere).unwrap(), b"keep");
}

/// Whoever serves the bucket cannot hand back an older signed manifest that
/// is still newer than what runs: the highest sequence a channel has shown
/// is kept with the update state, and anything below it is refused and never
/// downloaded, by this process and by the next. The other channel keeps its
/// own mark.
#[test]
fn a_manifest_below_the_highest_its_channel_showed_is_refused() {
    use super::channel::Updater;
    let bucket = Bucket::new();
    let (_root, layout) = rig();
    let running = ducktape_updater::Phase::Idle(ducktape_updater::Idle {
        current: Sha::digest(b"a"),
        previous: None,
        pinned_sequence: 1000,
    });
    super::persist_phase(&layout, &running).unwrap();
    let updater = |channel| {
        Updater::new(
            layout.clone(),
            running.clone(),
            keys(),
            channel,
            &bucket.base,
        )
    };
    // the channel shows 1010, whose archive holds no app: nothing stages
    publish(
        &bucket,
        Channel::Stable,
        1010,
        &[Item::File("README", b"no app".to_vec(), 0o644)],
    );
    let mut first = updater(Channel::Stable);
    block(first.check(&client(), Take::Automatic));
    assert_eq!(
        first.shown().status,
        "Verification refused: release_incomplete"
    );
    // then an older signed release, still newer than what runs
    let older = publish(&bucket, Channel::Stable, 1005, &release_of(script("o")));
    for mut checking in [first, updater(Channel::Stable)] {
        block(checking.check(&client(), Take::Automatic));
        assert_eq!(
            checking.shown().status,
            "Last check refused: sequence_replayed"
        );
    }
    let archive = layout::archive_name(&older, &Platform::HOST.key());
    assert_eq!(bucket.asked_for(&archive), 0, "never downloaded");
    assert_eq!(
        super::read_state(&layout.state_path()).unwrap(),
        Some(running.clone())
    );
    publish(&bucket, Channel::Nightly, 1005, &release_of(script("n")));
    super::channel::keep_channel(&layout, Channel::Nightly).unwrap();
    let mut nightly = updater(Channel::Nightly);
    block(nightly.check(&client(), Take::Automatic));
    assert_ne!(
        nightly.shown().status,
        "Last check refused: sequence_replayed"
    );
}

/// The running app and the boot step, on a Linux install: the layout the
/// tests lay out by hand is the Linux one (`current` and `previous` links).
#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};

    use ducktape_updater::{Event, Idle, Phase};

    use super::*;
    use crate::runtime::notify::{CenterHandle, HOST};
    use crate::update::channel::Updater;
    use crate::update::read_state;

    fn state(layout: &Layout) -> Phase {
        read_state(&layout.state_path()).unwrap().unwrap()
    }

    // ---- the running app, on a Linux install ------------------------------------

    /// Install `exe` (a script written to a scratch path, as the binary that
    /// runs `install`) and hand back its sha and the phase it pinned.
    fn install(root: &Path, layout: &Layout, exe: &[u8]) -> (Sha, Phase) {
        use std::os::unix::fs::PermissionsExt as _;
        let own = root.join("download").join(APP_EXE);
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, exe).unwrap();
        std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sha = crate::update::install::install(layout, &own).unwrap();
        (sha, state(layout))
    }

    fn updater(layout: &Layout, bucket: &Bucket, channel: Channel) -> Updater {
        Updater::new(layout.clone(), state(layout), keys(), channel, &bucket.base)
    }

    /// A newer release on the channel is downloaded, verified and staged,
    /// and waits for the next start; the pin moves only at the flip. What
    /// it lays down stays its user's to remove or replace.
    #[test]
    fn a_check_stages_a_newer_release_and_keeps_the_pin() {
        use std::os::unix::fs::PermissionsExt as _;
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let pin = installed.pinned_sequence();
        let b = publish(&bucket, Channel::Stable, pin + 1, &release_of(script("b")));
        let mut updater = updater(&layout, &bucket, Channel::Stable);
        updater.start();
        block(updater.check(&client(), Take::Automatic));
        let Phase::Staged(staged) = state(&layout) else {
            panic!("not staged: {:?}", state(&layout));
        };
        assert_eq!(
            (
                staged.current,
                staged.staged,
                staged.sequence,
                staged.pinned_sequence
            ),
            (a, b, pin + 1, pin)
        );
        assert_eq!(updater.phase(), &Phase::Staged(staged));
        assert!(
            !layout.releases_dir().join(format!("{b}.partial")).exists(),
            "the partial it was extracted from went"
        );
        for path in [layout.release_dir(b), layout.release_exe(b)] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o200, 0o200, "{} is read-only", path.display());
        }
        let shown = updater.shown();
        assert!(shown.restart);
        assert!(
            shown
                .status
                .ends_with("is ready: it takes over at the next start"),
            "{}",
            shown.status
        );
    }

    /// An archive that hashes right but holds no app is refused at verify: the
    /// pin and what runs stay, and Settings says why.
    #[test]
    fn an_archive_without_the_app_is_refused_and_nothing_moves() {
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (_, installed) = install(root.path(), &layout, &script("a"));
        let pin = installed.pinned_sequence();
        let b = publish(
            &bucket,
            Channel::Stable,
            pin + 1,
            &[Item::File("README", b"no app".to_vec(), 0o644)],
        );
        let mut updater = updater(&layout, &bucket, Channel::Stable);
        block(updater.check(&client(), Take::Automatic));
        assert_eq!(state(&layout), installed);
        assert!(!layout.release_dir(b).exists());
        assert_eq!(
            updater.shown().status,
            "Verification refused: release_incomplete"
        );
    }

    /// The channel switch, both ways. Nightly → Stable with stable older than
    /// what runs: the nightly staged before the switch is dropped and nothing
    /// older is taken; stable newer than what runs is. Stable → Nightly takes
    /// the newest nightly at the next check.
    #[test]
    fn a_channel_switch_never_goes_back_and_takes_the_other_channel_once_newer() {
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, _) = install(root.path(), &layout, &script("a"));
        // what runs, published at 1000 (a commit's time, as every release)
        let pin = 1000;
        let installed = Phase::Idle(Idle {
            current: a,
            previous: None,
            pinned_sequence: pin,
        });
        crate::update::persist_phase(&layout, &installed).unwrap();
        let behind = publish(&bucket, Channel::Stable, pin - 10, &release_of(script("s")));
        let nightly = publish(
            &bucket,
            Channel::Nightly,
            pin + 100,
            &release_of(script("n")),
        );
        // a pick is the channel file, then the running updater's switch
        let pick = |updater: &mut Updater, channel| {
            crate::update::channel::keep_channel(&layout, channel).unwrap();
            updater.switch(channel);
        };
        crate::update::channel::keep_channel(&layout, Channel::Nightly).unwrap();
        let mut updater = updater(&layout, &bucket, Channel::Nightly);
        block(updater.check(&client(), Take::Automatic));
        assert!(matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == nightly));

        pick(&mut updater, Channel::Stable);
        assert_eq!(
            state(&layout),
            Phase::Idle(Idle {
                current: a,
                previous: None,
                pinned_sequence: pin
            }),
            "the nightly staged before the switch is dropped"
        );
        assert!(!layout.release_dir(nightly).exists());
        block(updater.check(&client(), Take::Automatic));
        assert!(matches!(state(&layout), Phase::Idle(ref idle) if idle.current == a));
        let archive = layout::archive_name(&behind, &Platform::HOST.key());
        assert_eq!(
            bucket.asked_for(&archive),
            0,
            "an older stable is never downloaded"
        );
        assert_eq!(
            updater.shown().status,
            "Stable has nothing newer than this version yet: this one stays until it does"
        );
        let stable = publish(
            &bucket,
            Channel::Stable,
            pin + 200,
            &release_of(script("s2")),
        );
        block(updater.check(&client(), Take::Automatic));
        assert!(matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == stable));

        pick(&mut updater, Channel::Nightly);
        let newest = publish(
            &bucket,
            Channel::Nightly,
            pin + 300,
            &release_of(script("n2")),
        );
        block(updater.check(&client(), Take::Automatic));
        assert!(matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == newest));
    }

    /// Updating automatically off: a check that finds a newer release names
    /// it, downloads nothing, raises no notice, and the next start runs what
    /// it ran. "Update now" then downloads, verifies and stages it, a check
    /// while it waits leaves it be, and the next start flips to it as
    /// updating automatically would.
    #[test]
    fn with_updating_automatically_off_a_check_names_what_update_now_stages() {
        use crate::update::boot::{Boot, boot_at};
        use crate::update::channel::{keep_automatic, noticed_here};
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let b = publish(
            &bucket,
            Channel::Stable,
            installed.pinned_sequence() + 1,
            &release_of(script("b")),
        );
        keep_automatic(&layout, false).unwrap();
        noticed_here();

        let mut running = updater(&layout, &bucket, Channel::Stable);
        block(running.check(&client(), Take::Nothing));
        let archive = layout::archive_name(&b, &Platform::HOST.key());
        assert_eq!(bucket.asked_for(&archive), 0, "nothing downloaded");
        assert_eq!(state(&layout), installed);
        assert!(!layout.release_dir(b).exists());
        assert_eq!(noticed_here(), Vec::<String>::new(), "no notice");
        let shown = running.shown();
        assert_eq!(
            shown.status,
            format!("0.1.0+{} is available: it is not installed", b.short())
        );
        assert!(!shown.automatic && shown.update_now && !shown.restart);
        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Run(installed.clone())
        );

        // "Update now"
        block(running.check(&client(), Take::Asked));
        let Phase::Staged(staged) = state(&layout) else {
            panic!("not staged: {:?}", state(&layout));
        };
        assert_eq!((staged.staged, staged.asked), (b, true));
        let shown = running.shown();
        assert!(shown.restart && !shown.update_now);
        assert!(
            shown
                .status
                .ends_with("is ready: it takes over at the next start"),
            "{}",
            shown.status
        );
        assert_eq!(noticed_here(), [format!("ready 0.1.0+{}", b.short())]);
        let newer = publish(
            &bucket,
            Channel::Stable,
            installed.pinned_sequence() + 2,
            &release_of(script("c")),
        );
        running.apply(Event::AutomaticOff);
        block(running.check(&client(), Take::Nothing));
        assert!(matches!(state(&layout), Phase::Staged(ref waiting) if waiting.staged == b));
        let archive = layout::archive_name(&newer, &Platform::HOST.key());
        assert_eq!(bucket.asked_for(&archive), 0, "nothing newer downloaded");

        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Exec(layout.app_exe())
        );
        assert_eq!(current(&layout), Layout::link_target(b));
    }

    /// A release the updater staged on its own, then the switch went off:
    /// a start that comes before the running app's next turn drops it, and
    /// so does that turn; neither flips to it, and the next check names it
    /// as available, not installed.
    #[test]
    fn a_release_staged_before_the_switch_went_off_is_never_applied_unasked() {
        use crate::update::boot::{Boot, boot_at};
        use crate::update::channel::keep_automatic;
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let sequence = installed.pinned_sequence() + 1;
        let b = stage(&layout, &bucket, sequence, script("b"));
        keep_automatic(&layout, false).unwrap();
        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Run(installed.clone())
        );
        assert_eq!(current(&layout), Layout::link_target(a));
        assert!(!layout.release_dir(b).exists(), "the staged release went");

        keep_automatic(&layout, true).unwrap();
        let b = stage(&layout, &bucket, sequence, script("b"));
        keep_automatic(&layout, false).unwrap();
        let mut running = updater(&layout, &bucket, Channel::Stable);
        assert!(!running.shown().restart, "no restart offered");
        running.apply(Event::AutomaticOff);
        assert_eq!(state(&layout), installed);
        assert!(!layout.release_dir(b).exists());
        block(running.check(&client(), Take::Nothing));
        assert_eq!(
            running.shown().status,
            format!("0.1.0+{} is available: it is not installed", b.short())
        );
    }

    /// The switch goes off while a check updating automatically downloads:
    /// the download's answer is dropped, never staged, and the next turn's
    /// `AutomaticOff` collects what it left.
    #[test]
    fn the_switch_going_off_during_a_check_drops_what_it_was_downloading() {
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (_, installed) = install(root.path(), &layout, &script("a"));
        let b = publish(
            &bucket,
            Channel::Stable,
            installed.pinned_sequence() + 1,
            &release_of(script("b")),
        );
        let off = layout.clone();
        bucket.when_asked(
            &layout::archive_name(&b, &Platform::HOST.key()),
            move || crate::update::channel::keep_automatic(&off, false).unwrap(),
        );
        let mut running = updater(&layout, &bucket, Channel::Stable);
        block(running.check(&client(), Take::Automatic));
        assert!(
            !matches!(state(&layout), Phase::Staged(_)),
            "{:?}",
            state(&layout)
        );
        running.apply(Event::AutomaticOff);
        assert_eq!(state(&layout), installed);
        assert!(!layout.release_dir(b).exists());
    }

    // ---- the boot step, on a Linux install --------------------------------------

    /// Stage `exe` as the release after the installed one, through a check.
    fn stage(layout: &Layout, bucket: &Bucket, sequence: u64, exe: Vec<u8>) -> Sha {
        let sha = publish(bucket, Channel::Stable, sequence, &release_of(exe));
        let mut updater = updater(layout, bucket, Channel::Stable);
        block(updater.check(&client(), Take::Automatic));
        assert!(matches!(state(layout), Phase::Staged(_)));
        sha
    }

    fn current(layout: &Layout) -> PathBuf {
        std::fs::read_link(layout.current_link()).unwrap()
    }

    /// `cargo run` and a bare binary run with the updater off and touch
    /// nothing, with or without a `state.json` on disk.
    #[test]
    fn a_bare_binary_boots_with_the_updater_off() {
        use crate::update::boot::{Boot, boot_at};
        let (root, layout) = rig();
        let bare = root.path().join("target/debug").join(APP_EXE);
        std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
        std::fs::write(&bare, script("bare")).unwrap();
        assert_eq!(boot_at(&layout, &bare).unwrap(), Boot::Off);
        assert!(!layout.updates.exists(), "nothing written");
        let (_, installed) = install(root.path(), &layout, &script("a"));
        assert_eq!(boot_at(&layout, &bare).unwrap(), Boot::Off);
        assert_eq!(state(&layout), installed);
    }

    /// A staged release whose self-check fails never takes the install path:
    /// the boot runs what it ran, and the refusal is kept for Settings.
    #[test]
    fn a_staged_release_that_fails_its_self_check_is_never_flipped_to() {
        use crate::update::boot::{Boot, boot_at};
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        stage(
            &layout,
            &bucket,
            installed.pinned_sequence() + 1,
            b"#!/bin/sh\nexit 3\n".to_vec(),
        );
        let booted = boot_at(&layout, &layout.release_exe(a)).unwrap();
        let Boot::Run(Phase::Staged(staged)) = booted else {
            panic!("{booted:?}");
        };
        assert_eq!(staged.refused.as_deref(), Some("qualify_exit_3"));
        assert_eq!(current(&layout), Layout::link_target(a));
    }

    /// A staged release directory that became a link is not flipped to.
    #[test]
    fn a_staged_release_dir_that_became_a_link_is_not_flipped_to() {
        use crate::update::boot::{Boot, boot_at};
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let b = stage(
            &layout,
            &bucket,
            installed.pinned_sequence() + 1,
            script("b"),
        );
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::copy(layout.release_exe(b), elsewhere.join(APP_EXE)).unwrap();
        crate::update::remove_leftover(&layout.release_dir(b)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, layout.release_dir(b)).unwrap();
        let booted = boot_at(&layout, &layout.release_exe(a)).unwrap();
        let Boot::Run(Phase::Staged(staged)) = booted else {
            panic!("{booted:?}");
        };
        assert_eq!(staged.refused.as_deref(), Some("symlink_refused"));
        assert_eq!(current(&layout), Layout::link_target(a));
    }

    /// Adoption is macOS's: on Linux a binary inside the install with no
    /// state boots with the updater off and writes nothing.
    #[test]
    fn an_install_without_state_is_not_adopted_on_linux() {
        use std::os::unix::fs::PermissionsExt as _;

        use crate::update::boot::{Boot, boot_at};
        let (_root, layout) = rig();
        let a = Sha::digest(b"a");
        let exe = layout.release_exe(a);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, script("a")).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::update::replace_symlink(&layout.current_link(), &Layout::link_target(a)).unwrap();
        assert_eq!(boot_at(&layout, &exe).unwrap(), Boot::Off);
        assert!(!layout.updates.exists(), "nothing written");
    }

    /// Another process of this install holds its lock (on Linux each
    /// `duck://` link starts one): this start flips nothing, even with a
    /// release staged, and runs with the updater off; an install is refused
    /// until that process quits. The tree is laid by hand, so this process
    /// holds no lock of its own on it.
    #[test]
    fn a_second_process_of_the_install_touches_nothing() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::io::AsRawFd as _;

        use crate::update::boot::{Boot, boot_at};
        let (root, layout) = rig();
        let (a, b) = (Sha::digest(b"a"), Sha::digest(b"b"));
        for (sha, name) in [(a, "a"), (b, "b")] {
            let exe = layout.release_exe(sha);
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, script(name)).unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        crate::update::replace_symlink(&layout.current_link(), &Layout::link_target(a)).unwrap();
        let staged = Phase::Staged(ducktape_updater::Staged {
            current: a,
            previous: None,
            pinned_sequence: 10,
            staged: b,
            sequence: 11,
            display: "0.1.0+b".into(),
            channel: "stable".into(),
            refused: None,
            asked: false,
        });
        crate::update::persist_phase(&layout, &staged).unwrap();
        // the other process's hold, through an open file of its own
        let other = std::fs::File::create(layout.updates.join("lock")).unwrap();
        // SAFETY: an open descriptor and flag constants.
        let held = unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(held, 0);

        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Elsewhere
        );
        assert_eq!(current(&layout), Layout::link_target(a));
        assert_eq!(state(&layout), staged);
        let own = root.path().join(APP_EXE);
        std::fs::write(&own, script("c")).unwrap();
        let refused = crate::update::install::install(&layout, &own).unwrap_err();
        assert_eq!(refused.reason, "install_busy");
        assert_eq!(state(&layout), staged);
    }

    /// The person picks nightly while a stable check is downloading: the
    /// download's answer is the old channel's, so it is dropped, never
    /// staged, and the next turn's switch collects what it left. Nothing is
    /// offered for a restart, and the next start runs what it ran.
    #[test]
    fn a_pick_during_a_check_drops_what_the_old_channel_was_downloading() {
        use crate::update::boot::{Boot, boot_at};
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let b = publish(
            &bucket,
            Channel::Stable,
            installed.pinned_sequence() + 1,
            &release_of(script("b")),
        );
        let picked = layout.clone();
        bucket.when_asked(
            &layout::archive_name(&b, &Platform::HOST.key()),
            move || crate::update::channel::keep_channel(&picked, Channel::Nightly).unwrap(),
        );
        let mut updater = updater(&layout, &bucket, Channel::Stable);
        updater.start();
        block(updater.check(&client(), Take::Automatic));
        assert!(
            !matches!(state(&layout), Phase::Staged(_)),
            "{:?}",
            state(&layout)
        );
        assert!(!updater.shown().restart, "no restart offered");
        updater.switch(Channel::Nightly);
        assert_eq!(state(&layout), installed);
        assert!(!layout.release_dir(b).exists());
        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Run(installed)
        );
    }

    /// A release stable staged, then the pick of nightly, then a start
    /// before the running app's switch dropped it (Restart pressed at
    /// once): the boot step drops the old channel's release, never flips.
    #[test]
    fn a_start_never_flips_to_a_release_another_channel_staged() {
        use crate::update::boot::{Boot, boot_at};
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let b = stage(
            &layout,
            &bucket,
            installed.pinned_sequence() + 1,
            script("b"),
        );
        crate::update::channel::keep_channel(&layout, Channel::Nightly).unwrap();
        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Run(installed.clone())
        );
        assert_eq!(state(&layout), installed);
        assert_eq!(current(&layout), Layout::link_target(a));
        assert!(!layout.release_dir(b).exists(), "the staged release went");
    }

    /// A `state.json` that is a link is not followed: this start runs with the
    /// updater off.
    #[test]
    fn a_linked_state_file_is_refused() {
        use crate::update::boot::boot_at;
        let (root, layout) = rig();
        let (a, _) = install(root.path(), &layout, &script("a"));
        let moved = root.path().join("state.json");
        std::fs::rename(layout.state_path(), &moved).unwrap();
        std::os::unix::fs::symlink(&moved, layout.state_path()).unwrap();
        let refused = boot_at(&layout, &layout.release_exe(a)).unwrap_err();
        assert_eq!(refused.reason, "symlink_refused");
    }

    /// A `state.json` that does not parse (another build's shape) is
    /// rebuilt from the release the install path names, as an install would
    /// have written it: a staged release is dropped, `seen.json` stays. A
    /// process that is not that release rebuilds nothing.
    #[test]
    fn a_state_that_does_not_parse_is_rebuilt_from_the_install_path() {
        use crate::update::boot::{Boot, boot_at};
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let b = stage(
            &layout,
            &bucket,
            installed.pinned_sequence() + 1,
            script("b"),
        );
        let seen = std::fs::read(layout.seen_path()).unwrap();
        std::fs::write(layout.state_path(), r#"{"phase":"from another build"}"#).unwrap();

        let refused = boot_at(&layout, &layout.release_exe(b)).unwrap_err();
        assert_eq!(refused.reason, "state_invalid");
        assert!(layout.release_dir(b).exists(), "nothing touched");

        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Run(installed.clone())
        );
        assert_eq!(state(&layout), installed);
        assert_eq!(current(&layout), Layout::link_target(a));
        assert!(
            !layout.release_dir(b).exists(),
            "the staged release is dropped"
        );
        assert_eq!(std::fs::read(layout.seen_path()).unwrap(), seen);
    }

    /// A crash between `Swapping` and `PendingHealthy` finishes from whichever
    /// side the install path is on: landed, it records `previous`; untouched,
    /// it flips. Either way the start runs the new release.
    #[test]
    fn a_crash_mid_flip_finishes_from_whichever_side_landed() {
        use crate::update::boot::{Boot, boot_at};
        for landed in [true, false] {
            let bucket = Bucket::new();
            let (root, layout) = rig();
            let (a, installed) = install(root.path(), &layout, &script("a"));
            let pin = installed.pinned_sequence();
            let b = stage(&layout, &bucket, pin + 1, script("b"));
            let swapping = Phase::Swapping(ducktape_updater::Swapping {
                from: a,
                to: b,
                pinned_sequence: pin + 1,
                display: format!("0.1.0+{}", b.short()),
            });
            crate::update::persist_phase(&layout, &swapping).unwrap();
            if landed {
                crate::update::replace_symlink(&layout.current_link(), &Layout::link_target(b))
                    .unwrap();
            }
            let booted = boot_at(&layout, &layout.release_exe(a)).unwrap();
            assert_eq!(booted, Boot::Exec(layout.app_exe()), "landed: {landed}");
            assert_eq!(current(&layout), Layout::link_target(b));
            assert_eq!(
                std::fs::read_link(layout.previous_link()).unwrap(),
                Layout::link_target(a)
            );
            assert!(matches!(
                state(&layout),
                Phase::PendingHealthy(ref pending) if pending.boots == 0 && pending.current == b
            ));
        }
    }

    /// An install lays a clean set whatever it finds: the releases an update
    /// left, a partial, `previous`, all gone; the desktop entry names the
    /// install path.
    #[test]
    fn a_second_install_lays_a_clean_set() {
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        stage(
            &layout,
            &bucket,
            installed.pinned_sequence() + 1,
            script("b"),
        );
        std::fs::write(layout.partial_path(Sha::digest(b"p")), b"part").unwrap();
        crate::update::replace_symlink(&layout.previous_link(), &Layout::link_target(a)).unwrap();
        let (c, reinstalled) = install(root.path(), &layout, &script("c"));
        let releases: Vec<_> = std::fs::read_dir(layout.releases_dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(releases, vec![c.to_string()]);
        assert!(std::fs::symlink_metadata(layout.previous_link()).is_err());
        assert_eq!(current(&layout), Layout::link_target(c));
        assert_eq!(
            reinstalled,
            Phase::Idle(Idle {
                current: c,
                previous: None,
                pinned_sequence: installed.pinned_sequence()
            })
        );
        let entry = std::fs::read_to_string(layout.desktop_entry()).unwrap();
        assert!(
            entry.contains(&format!("Exec=\"{}\" %u", layout.app_exe().display())),
            "{entry}"
        );
        assert!(layout.icon().is_file());
    }

    /// An install root this user cannot write is refused, never half laid.
    #[test]
    fn an_install_root_this_user_cannot_write_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        // SAFETY: a plain getter.
        if unsafe { libc::geteuid() } == 0 {
            return; // root writes anywhere
        }
        let (root, layout) = rig();
        std::fs::create_dir_all(&layout.install).unwrap();
        std::fs::set_permissions(&layout.install, std::fs::Permissions::from_mode(0o555)).unwrap();
        let own = root.path().join(APP_EXE);
        std::fs::write(&own, script("a")).unwrap();
        let refused = crate::update::install::install(&layout, &own).unwrap_err();
        std::fs::set_permissions(&layout.install, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(refused.reason, "install_root_not_writable");
        assert!(!layout.state_path().exists());
    }

    /// The flip came up: its first frame settles it, keeping what runs and the
    /// release before it, and collects the rest.
    #[test]
    fn a_flip_that_renders_settles_and_collects_the_rest() {
        use crate::update::boot::{Boot, boot_at};
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let b = stage(
            &layout,
            &bucket,
            installed.pinned_sequence() + 1,
            script("b"),
        );
        let stray = layout.release_dir(Sha::digest(b"stray"));
        std::fs::create_dir_all(&stray).unwrap();
        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Exec(layout.app_exe())
        );
        let booted = boot_at(&layout, &layout.release_exe(b)).unwrap();
        let Boot::Run(phase) = booted else {
            panic!("{booted:?}");
        };
        let mut updater =
            Updater::new(layout.clone(), phase, keys(), Channel::Stable, &bucket.base);
        updater.start();
        assert_eq!(
            state(&layout),
            Phase::Idle(Idle {
                current: b,
                previous: Some(a),
                pinned_sequence: installed.pinned_sequence() + 1
            })
        );
        assert!(!stray.exists());
        assert!(layout.release_dir(a).exists() && layout.release_dir(b).exists());
    }

    /// The rows the update notices left in `center`, newest first: each
    /// one's program, title and whether it was read.
    fn rows(center: &CenterHandle) -> Vec<(String, String, bool)> {
        center
            .lock()
            .entries()
            .map(|entry| (entry.program.clone(), entry.title.clone(), entry.read))
            .collect()
    }

    /// The "is ready" notice stands exactly while a release is staged, its
    /// row in the centre with it: up as one is, both down when a switch
    /// drops it; a start that still has it staged leaves it up. The first
    /// frame after the flip says once what it installed, in the ready one's
    /// place, and the start after that says nothing new.
    #[test]
    fn the_ready_notice_stands_while_a_release_is_staged_and_goes_with_it() {
        use crate::update::boot::{Boot, boot_at};
        use crate::update::channel::noticed_here;
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let pin = installed.pinned_sequence();
        let b = publish(&bucket, Channel::Stable, pin + 1, &release_of(script("b")));
        let ready = format!("ready 0.1.0+{}", b.short());
        let ready_row = |read| {
            let title = format!("Ducktape 0.1.0+{} is ready.", b.short());
            vec![(HOST.to_owned(), title, read)]
        };
        let center = CenterHandle::default();
        noticed_here();

        let mut running = updater(&layout, &bucket, Channel::Stable).with_center(center.clone());
        running.start();
        assert_eq!(
            rows(&center),
            [],
            "a start with nothing staged says nothing"
        );
        block(running.check(&client(), Take::Automatic));
        assert_eq!(noticed_here(), ["withdrawn", ready.as_str()]);
        assert_eq!(rows(&center), ready_row(false), "the staged release's row");

        crate::update::channel::keep_channel(&layout, Channel::Nightly).unwrap();
        running.switch(Channel::Nightly);
        assert_eq!(noticed_here(), ["withdrawn"], "the switch dropped it");
        assert_eq!(rows(&center), [], "and its row");

        crate::update::channel::keep_channel(&layout, Channel::Stable).unwrap();
        let mut running = updater(&layout, &bucket, Channel::Stable).with_center(center.clone());
        block(running.check(&client(), Take::Automatic));
        assert_eq!(noticed_here(), [ready.as_str()]);
        updater(&layout, &bucket, Channel::Stable)
            .with_center(center.clone())
            .start();
        assert_eq!(noticed_here(), Vec::<String>::new(), "still staged");
        assert_eq!(rows(&center), ready_row(false), "its row stands");

        assert_eq!(
            boot_at(&layout, &layout.release_exe(a)).unwrap(),
            Boot::Exec(layout.app_exe())
        );
        let Boot::Run(phase) = boot_at(&layout, &layout.release_exe(b)).unwrap() else {
            panic!("B does not run");
        };
        Updater::new(layout.clone(), phase, keys(), Channel::Stable, &bucket.base)
            .with_center(center.clone())
            .start();
        let installed = format!("Ducktape {} is installed.", crate::update::version());
        assert_eq!(
            noticed_here(),
            [format!("installed {}", crate::update::version())],
            "the flip took it over"
        );
        assert_eq!(
            rows(&center),
            [(HOST.to_owned(), installed.clone(), false)],
            "in the ready one's place"
        );

        updater(&layout, &bucket, Channel::Stable)
            .with_center(center.clone())
            .start();
        assert_eq!(
            noticed_here(),
            ["withdrawn"],
            "the next start says nothing new"
        );
        assert_eq!(rows(&center), [(HOST.to_owned(), installed, false)], "once");
    }

    /// A check says when it ended: one the channel does not answer says
    /// so, one that finds nothing newer leaves the status as it was and
    /// says it ran.
    #[test]
    fn a_check_says_when_it_ended_and_whether_the_channel_answered() {
        use crate::update::channel::clock;
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (_, installed) = install(root.path(), &layout, &script("a"));
        let mut running = updater(&layout, &bucket, Channel::Stable);
        running.start();
        assert_eq!(running.shown().checked, None, "not checked yet");
        // the minute it ended, read on either side of it
        let check = |running: &mut Updater| {
            let before = clock(crate::runtime::notify::wall());
            block(running.check(&client(), Take::Automatic));
            let after = clock(crate::runtime::notify::wall());
            (running.shown(), [before, after])
        };

        // nothing published: the channel did not answer
        let (shown, at) = check(&mut running);
        assert_eq!(shown.status, "Not checked yet");
        let checked = shown.checked.unwrap();
        assert!(
            at.iter()
                .any(|at| checked == format!("Stable did not answer at {at}")),
            "{checked}"
        );

        // what runs is what the channel offers: up to date, and checked
        publish(
            &bucket,
            Channel::Stable,
            installed.pinned_sequence(),
            &release_of(script("a")),
        );
        let (shown, at) = check(&mut running);
        assert_eq!(shown.status, "Up to date");
        let checked = shown.checked.unwrap();
        assert!(
            at.iter().any(|at| checked == format!("checked at {at}")),
            "{checked}"
        );
        let status = shown.status;
        let (again, _) = check(&mut running);
        assert_eq!(again.status, status, "the words stay; the time says it ran");
        assert!(again.checked.is_some());
    }

    /// The headline. An installed app checks the channel, finds a newer signed
    /// release and stages it; the next start flips to it and runs it. That
    /// release never renders a frame: the start after flips back to the one
    /// before, which runs, reads the rollback, and never downloads the failed
    /// release again.
    #[test]
    fn an_installed_app_runs_a_newer_signed_release_next_start_and_one_that_never_renders_is_rolled_back()
     {
        use crate::update::boot::{Boot, boot_at};
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let (own_a, pin) = (layout.release_exe(a), installed.pinned_sequence());

        // start 1: A is what the install path names; it runs, renders, checks
        let booted = boot_at(&layout, &own_a).unwrap();
        assert_eq!(booted, Boot::Run(installed.clone()));
        let b = publish(&bucket, Channel::Stable, pin + 1, &release_of(script("b")));
        let center = CenterHandle::default();
        let mut running = updater(&layout, &bucket, Channel::Stable).with_center(center.clone());
        running.start();
        block(running.check(&client(), Take::Automatic));
        assert!(matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == b));
        assert_eq!(rows(&center).len(), 1, "B's ready row");

        // start 2: the session starts the install path's A; its boot step
        // qualifies B, flips, and hands the process to B
        assert_eq!(
            boot_at(&layout, &own_a).unwrap(),
            Boot::Exec(layout.app_exe())
        );
        assert_eq!(current(&layout), Layout::link_target(b));
        let own_b = layout.release_exe(b);
        // B's own boot step counts itself and goes on as the app
        let booted = boot_at(&layout, &own_b).unwrap();
        let Boot::Run(Phase::PendingHealthy(pending)) = booted else {
            panic!("{booted:?}");
        };
        assert_eq!(
            (pending.current, pending.previous, pending.boots),
            (b, a, 1)
        );
        assert_eq!(pending.pinned_sequence, pin + 1);
        // ... and B never renders a frame

        // start 3: the install path names B; its boot step flips back and
        // hands the process to A
        assert_eq!(
            boot_at(&layout, &own_b).unwrap(),
            Boot::Exec(layout.app_exe())
        );
        assert_eq!(current(&layout), Layout::link_target(a));
        let booted = boot_at(&layout, &own_a).unwrap();
        let Boot::Run(phase @ Phase::RolledBack(_)) = booted else {
            panic!("{booted:?}");
        };
        // what an earlier flip left, and a download that never finished
        let earlier = layout.release_dir(Sha::digest(b"earlier"));
        std::fs::create_dir_all(&earlier).unwrap();
        let partial = layout.releases_dir().join(format!("{b}.partial"));
        std::fs::write(&partial, b"part").unwrap();

        // A runs, reads the rollback, and the channel's B is not taken again;
        // B's row goes, and nothing new is said (Settings says it)
        let mut running =
            Updater::new(layout.clone(), phase, keys(), Channel::Stable, &bucket.base)
                .with_center(center.clone());
        running.start();
        assert_eq!(
            running.shown().status,
            format!("Went back from 0.1.0+{}: it never came up", b.short())
        );
        assert_eq!(rows(&center), [], "B's row went with it");
        block(running.check(&client(), Take::Automatic));
        assert_eq!(
            state(&layout),
            Phase::Idle(Idle {
                current: a,
                previous: None,
                pinned_sequence: pin + 1
            })
        );
        let archive = layout::archive_name(&b, &Platform::HOST.key());
        assert_eq!(bucket.asked_for(&archive), 1, "B was downloaded once");
        // the rollback's leftovers went: only what runs is on disk
        let left: Vec<_> = std::fs::read_dir(layout.releases_dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from(a.to_string())]);
        assert!(std::fs::symlink_metadata(layout.previous_link()).is_err());
    }
}

/// The same flow on a macOS install, with real bundles: tiny Mach-O apps
/// built with `cc`, ad-hoc signed, published as the release job packs
/// `Ducktape.app` (bsdtar, no AppleDouble files, no extended attributes),
/// staged under `codesign --verify`, and flipped by the bundle swap.
#[cfg(target_os = "macos")]
mod macos {
    use std::path::Path;
    use std::process::Command;

    use ducktape_updater::{Idle, Phase};

    use super::*;
    use crate::update::boot::{Boot, boot_at};
    use crate::update::channel::Updater;
    use crate::update::{BUNDLE, bundle_exe, read_state};

    fn state(layout: &Layout) -> Phase {
        read_state(&layout.state_path()).unwrap().unwrap()
    }

    fn run(command: &mut Command) {
        let status = command.status().unwrap();
        assert!(status.success(), "{command:?}: {status}");
    }

    /// `<dir>/Ducktape.app`: an app that answers `--version` as `name`,
    /// ad-hoc signed.
    fn bundle(dir: &Path, name: &str) -> std::path::PathBuf {
        let bundle = dir.join(BUNDLE);
        let exe = bundle_exe(&bundle);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let source = dir.join("app.c");
        std::fs::write(
            &source,
            format!(
                "#include <stdio.h>\n#include <string.h>\nint main(int c, char **v) {{ if (c > 1 && !strcmp(v[1], \"--version\")) puts(\"ducktape-app 0.0.0+{name}\"); return 0; }}\n"
            ),
        )
        .unwrap();
        run(Command::new("cc").arg(&source).arg("-o").arg(&exe));
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleExecutable</key><string>{APP_EXE}</string><key>CFBundleIdentifier</key><string>dev.ducktape.test.{name}</string></dict></plist>\n"
            ),
        )
        .unwrap();
        run(Command::new("/usr/bin/codesign")
            .args(["-s", "-", "--force", "--deep"])
            .arg(&bundle));
        bundle
    }

    /// The release job's archive of a bundle.
    fn pack(bundle: &Path) -> Vec<u8> {
        let archive = bundle.with_extension("tar.gz");
        run(Command::new("tar")
            .env("COPYFILE_DISABLE", "1")
            .args(["--no-mac-metadata", "--no-xattrs", "-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(bundle.parent().unwrap())
            .arg(BUNDLE));
        std::fs::read(archive).unwrap()
    }

    /// `bytes` published as the archive of a release on stable at
    /// `sequence`; the archive's sha.
    fn publish_archive(bucket: &Bucket, sequence: u64, bytes: Vec<u8>) -> Sha {
        let sha = Sha::digest(&bytes);
        let manifest = Manifest {
            schema: SCHEMA,
            channel: "stable".into(),
            sequence,
            published_at: "2026-10-13T00:00:00Z".into(),
            release: Release {
                sha256_id: Sha::ZERO,
                display: format!("0.1.0+{}", sha.short()),
                notes_url: String::new(),
            },
            artifacts: BTreeMap::from([(
                Platform::HOST.key(),
                Artifact {
                    sha256: sha,
                    size: bytes.len() as u64,
                },
            )]),
            successor_key: None,
        }
        .sealed();
        bucket.put(&layout::archive_name(&sha, &Platform::HOST.key()), bytes);
        bucket.put("stable.json", channel_file(&manifest));
        sha
    }

    /// What a start adopts: the bundle that runs, nothing to go back to,
    /// pinned at this build's own sequence.
    fn adopted(own: &Path) -> Phase {
        Phase::Idle(Idle {
            current: crate::update::digest_file(own).unwrap(),
            previous: None,
            pinned_sequence: crate::update::own_sequence(),
        })
    }

    /// A bundle dragged into the install dir from the DMG has no update
    /// state: its first start adopts it as `install` would have laid it. The
    /// same bundle run from anywhere else (the mounted DMG, a translocated
    /// copy) stays off and writes nothing.
    #[test]
    fn a_bundle_dragged_into_the_install_dir_adopts_itself() {
        let (root, layout) = rig();
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let away = bundle_exe(&bundle(&elsewhere, "a"));
        assert_eq!(boot_at(&layout, &away).unwrap(), Boot::Off);
        assert!(!layout.updates.exists(), "nothing written");

        std::fs::create_dir_all(&layout.install).unwrap();
        bundle(&layout.install, "a");
        let own = layout.app_exe();
        assert_eq!(boot_at(&layout, &own).unwrap(), Boot::Run(adopted(&own)));
        assert_eq!(state(&layout), adopted(&own));
        // the next start reads what this one wrote
        assert_eq!(boot_at(&layout, &own).unwrap(), Boot::Run(adopted(&own)));
        // and one that finds it unparsable adopts the bundle again
        std::fs::write(layout.state_path(), "{").unwrap();
        assert_eq!(boot_at(&layout, &own).unwrap(), Boot::Run(adopted(&own)));
        assert_eq!(state(&layout), adopted(&own));
    }

    /// A bundle dragged over an installed one (from a DMG, older or newer)
    /// is not the release the state names: the start adopts it, and a
    /// release the old state had staged is dropped, never swapped in over it.
    #[test]
    fn a_bundle_dragged_over_an_install_is_adopted() {
        let (root, layout) = rig();
        let download = root.path().join("download");
        std::fs::create_dir_all(&download).unwrap();
        let a =
            crate::update::install::install(&layout, &bundle_exe(&bundle(&download, "a"))).unwrap();
        let own = layout.app_exe();
        let older = Sha::digest(b"older");
        bundle(&layout.release_dir(older), "older");
        let staged = Phase::Staged(ducktape_updater::Staged {
            current: a,
            previous: None,
            pinned_sequence: crate::update::own_sequence(),
            staged: older,
            sequence: crate::update::own_sequence() + 1,
            display: "0.1.0+older".into(),
            channel: "stable".into(),
            refused: None,
            asked: false,
        });
        crate::update::persist_phase(&layout, &staged).unwrap();
        bundle(&layout.install, "n");
        assert_eq!(boot_at(&layout, &own).unwrap(), Boot::Run(adopted(&own)));
        assert!(
            !layout.release_dir(older).exists(),
            "the staged release is dropped"
        );
        let output = Command::new(&own).arg("--version").output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "ducktape-app 0.0.0+n"
        );
    }

    /// The Mac live test's F3: an older bundle dragged over an updated
    /// install. The state names a release staged from an archive, so the
    /// bundle the update put in place is known by the executable recorded
    /// as it staged: it starts as itself, and the older one over it is
    /// adopted, so the channel's newer releases are offered to it again.
    #[test]
    fn an_older_bundle_dragged_over_an_updated_install_is_adopted() {
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let download = root.path().join("download");
        std::fs::create_dir_all(&download).unwrap();
        crate::update::install::install(&layout, &bundle_exe(&bundle(&download, "a"))).unwrap();
        let pin = state(&layout).pinned_sequence();
        let built = root.path().join("built");
        std::fs::create_dir_all(&built).unwrap();
        let b = publish_archive(&bucket, pin + 1, pack(&bundle(&built, "b")));
        let updater =
            |phase| Updater::new(layout.clone(), phase, keys(), Channel::Stable, &bucket.base);
        let mut running = updater(state(&layout));
        running.start();
        block(running.check(&client(), Take::Automatic));
        let own = layout.app_exe();
        assert_eq!(boot_at(&layout, &own).unwrap(), Boot::Exec(own.clone()));
        let booted = boot_at(&layout, &own).unwrap();
        let Boot::Run(phase @ Phase::PendingHealthy(_)) = booted else {
            panic!("{booted:?}");
        };
        updater(phase).start();
        let updated = state(&layout);
        assert!(
            matches!(updated, Phase::Idle(ref idle) if idle.current == b),
            "{updated:?}"
        );
        assert_eq!(boot_at(&layout, &own).unwrap(), Boot::Run(updated));

        bundle(&layout.install, "old");
        assert_eq!(boot_at(&layout, &own).unwrap(), Boot::Run(adopted(&own)));
    }

    /// The headline on macOS: install, stage a newer signed bundle, the next
    /// start swaps it in, it never renders, the start after swaps back.
    #[test]
    fn an_installed_bundle_runs_a_newer_signed_bundle_next_start_and_one_that_never_renders_is_swapped_back()
     {
        let (root, layout) = rig();
        swap_in_and_back(root.path(), &layout);
        // nothing the updates laid down needs a chmod to go
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    /// LaunchServices knows one app of the install, the installed bundle: a
    /// staged bundle is never registered, and after a swap in and a swap
    /// back no parked or collected bundle is left registered. It writes to this Mac's LaunchServices,
    /// so it runs by hand, in `~/tmp` (where Spotlight looks), and
    /// unregisters what it leaves.
    #[test]
    #[ignore = "registers apps with this Mac's LaunchServices"]
    fn launch_services_knows_only_the_installed_bundle() {
        let tmp = Path::new(&std::env::var_os("HOME").unwrap()).join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        // not a dot dir: Spotlight skips hidden ones
        let root = tempfile::Builder::new()
            .prefix("ls-check-")
            .tempdir_in(&tmp)
            .unwrap();
        let layout = Layout {
            updates: root.path().join("cfg/ducktape/updates"),
            install: root.path().join("apps"),
            data_home: root.path().join("data"),
        };
        crate::update::macos::LAUNCH_SERVICES.with(|on| on.set(true));
        // a bundle that appears where Spotlight looks is registered within
        // 10-30 s on the test Mac; a staged one is given a minute
        let staged = Sha::digest(b"staged");
        bundle(&layout.release_dir(staged), "staged");
        std::thread::sleep(std::time::Duration::from_secs(60));
        let staged_registered = registered_under(root.path());
        crate::update::remove_leftover(&layout.release_dir(staged)).unwrap();
        // the flow's own builds stay out of the indexer's way
        swap_in_and_back(&root.path().join("builds.noindex"), &layout);
        let registered = registered_under(root.path());
        for bundle in &registered {
            crate::update::macos::launch_services(bundle, false);
        }
        for bundle in &staged_registered {
            crate::update::macos::launch_services(bundle, false);
        }
        let left = registered_under(root.path());
        std::fs::remove_dir_all(root.path()).unwrap();
        assert_eq!(staged_registered, Vec::<std::path::PathBuf>::new());
        let ours = |path: &&std::path::PathBuf| {
            path.starts_with(&layout.updates) || path.starts_with(&layout.install)
        };
        assert_eq!(
            registered.iter().filter(ours).collect::<Vec<_>>(),
            [&layout.installed_bundle()]
        );
        assert_eq!(left, Vec::<std::path::PathBuf>::new());
    }

    /// Every app LaunchServices has registered under `root`.
    fn registered_under(root: &Path) -> Vec<std::path::PathBuf> {
        let dump = Command::new(
            "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister",
        )
        .arg("-dump")
        .output()
        .unwrap();
        let mut paths: Vec<_> = String::from_utf8_lossy(&dump.stdout)
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("path:"))
            .map(|path| {
                path.trim()
                    .rsplit_once(" (0x")
                    .map_or(path.trim(), |(path, _)| path)
            })
            .map(std::path::PathBuf::from)
            .filter(|path| path.starts_with(root))
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    /// An installed A takes a newer signed B at its next start; B never
    /// renders, and the start after swaps A back in and collects B.
    fn swap_in_and_back(root: &Path, layout: &Layout) {
        let layout = layout.clone();
        let bucket = Bucket::new();
        let download = root.join("download");
        std::fs::create_dir_all(&download).unwrap();
        let own = bundle_exe(&bundle(&download, "a"));
        let a = crate::update::install::install(&layout, &own).unwrap();
        let installed = state(&layout);
        let pin = installed.pinned_sequence();
        let own_a = layout.app_exe();
        assert_eq!(boot_at(&layout, &own_a).unwrap(), Boot::Run(installed));

        let built = root.join("built");
        std::fs::create_dir_all(&built).unwrap();
        let b = publish_archive(&bucket, pin + 1, pack(&bundle(&built, "b")));
        let mut running = Updater::new(
            layout.clone(),
            state(&layout),
            keys(),
            Channel::Stable,
            &bucket.base,
        );
        running.start();
        block(running.check(&client(), Take::Automatic));
        assert!(
            matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == b && staged.refused.is_none()),
            "{:?}",
            state(&layout)
        );
        assert_eq!(running.phase(), &state(&layout));
        assert!(running.shown().restart, "Settings offers the restart");

        // the next start: A qualifies B, swaps it in, hands over
        assert_eq!(
            boot_at(&layout, &own_a).unwrap(),
            Boot::Exec(layout.app_exe())
        );
        let output = Command::new(layout.app_exe())
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "ducktape-app 0.0.0+b"
        );
        // the bundle the update put in place is its user's, as a dragged
        // one is: a Finder "Replace" or a removal needs no chmod
        let probe = layout.installed_bundle().join("Contents/MacOS/probe");
        std::fs::write(&probe, b"x").unwrap();
        std::fs::remove_file(&probe).unwrap();
        let own_b = layout.app_exe();
        let booted = boot_at(&layout, &own_b).unwrap();
        assert!(
            matches!(booted, Boot::Run(Phase::PendingHealthy(ref pending)) if pending.boots == 1),
            "{booted:?}"
        );

        // B never rendered: the start after swaps A back in
        assert_eq!(
            boot_at(&layout, &own_b).unwrap(),
            Boot::Exec(layout.app_exe())
        );
        let output = Command::new(layout.app_exe())
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "ducktape-app 0.0.0+a"
        );
        let booted = boot_at(&layout, &layout.app_exe()).unwrap();
        let Boot::Run(phase @ Phase::RolledBack(_)) = booted else {
            panic!("{booted:?}");
        };
        let mut running =
            Updater::new(layout.clone(), phase, keys(), Channel::Stable, &bucket.base);
        running.start();
        block(running.check(&client(), Take::Automatic));
        assert_eq!(
            state(&layout),
            Phase::Idle(Idle {
                current: a,
                previous: None,
                pinned_sequence: pin + 1
            })
        );
        let archive = layout::archive_name(&b, &Platform::HOST.key());
        assert_eq!(bucket.asked_for(&archive), 1, "B was downloaded once");
        // the failed bundle and its partial went: A is installed, and
        // nothing is kept beside it
        assert_eq!(std::fs::read_dir(layout.releases_dir()).unwrap().count(), 0);
        assert!(std::fs::symlink_metadata(layout.previous_link()).is_err());
    }
}
