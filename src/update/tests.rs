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

use app_update::layout::Channel;
use app_update::release::signing_key_from_hex;
use app_update::{
    Artifact, Manifest, Platform, PublicKey, Refusal as ManifestRefusal, Release, SCHEMA, Sha,
    Signature, TrustedKeys, layout,
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

/// A bucket on loopback: files by name, every request recorded.
struct Bucket {
    base: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    asked: Asked,
    /// Answer every request whole, as a server without range support does.
    ignore_range: Arc<AtomicBool>,
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
        };
        let (files, asked, ignore_range) = (
            bucket.files.clone(),
            bucket.asked.clone(),
            bucket.ignore_range.clone(),
        );
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &files, &asked, &ignore_range);
            }
        });
        bucket
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
    let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let signature = Signature::sign(&signing_key_from_hex(&seed()).unwrap(), &bytes).encoded();
    bucket.put(&layout::archive_name(&sha, &Platform::HOST.key()), archive);
    bucket.put(&channel.manifest(), bytes);
    bucket.put(&channel.signature(), signature.into_bytes());
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

// ---- the channel: fetch and download ----------------------------------------

/// Each channel's pair is read from its own files and verifies under the
/// pinned key; a stranger's signature over the same bytes is refused, and a
/// channel with nothing published is a check that did not happen, not a
/// refusal.
#[test]
fn a_published_pair_verifies_and_a_strangers_signature_is_refused() {
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
    let manifest = bucket.files.lock().unwrap()["stable.json"].clone();
    let stranger = signing_key_from_hex(&"07".repeat(32)).unwrap();
    bucket.put(
        "stable.json.sig",
        Signature::sign(&stranger, &manifest).encoded().into_bytes(),
    );
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

/// A manifest past the cap is not read, whatever it holds.
#[test]
fn a_manifest_larger_than_the_cap_is_not_read() {
    let bucket = Bucket::new();
    bucket.put("stable.json", vec![b' '; 300 * 1024]);
    bucket.put("stable.json.sig", b"00".to_vec());
    assert_eq!(
        block(fetch(&client(), &bucket.base, Channel::Stable, &keys())),
        None
    );
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
    let running = app_update::Phase::Idle(app_update::Idle {
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
    block(first.check(&client()));
    assert_eq!(
        first.shown().status,
        "Verification refused: release_incomplete"
    );
    // then an older signed release, still newer than what runs
    let older = publish(&bucket, Channel::Stable, 1005, &release_of(script("o")));
    for mut checking in [first, updater(Channel::Stable)] {
        block(checking.check(&client()));
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
    let mut nightly = updater(Channel::Nightly);
    block(nightly.check(&client()));
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

    use app_update::{Idle, Phase};

    use super::*;
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

    /// A newer release on the channel is downloaded, verified, staged sealed,
    /// and waits for the next start; the pin moves only at the flip.
    #[test]
    fn a_check_stages_a_newer_release_sealed_and_keeps_the_pin() {
        use std::os::unix::fs::PermissionsExt as _;
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let (a, installed) = install(root.path(), &layout, &script("a"));
        let pin = installed.pinned_sequence();
        let b = publish(&bucket, Channel::Stable, pin + 1, &release_of(script("b")));
        let mut updater = updater(&layout, &bucket, Channel::Stable);
        updater.start();
        block(updater.check(&client()));
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
        let mode = std::fs::metadata(layout.release_dir(b))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o222, 0, "the staged release is sealed");
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
        block(updater.check(&client()));
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
        let mut updater = updater(&layout, &bucket, Channel::Nightly);
        block(updater.check(&client()));
        assert!(matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == nightly));

        updater.switch(Channel::Stable);
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
        block(updater.check(&client()));
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
        block(updater.check(&client()));
        assert!(matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == stable));

        updater.switch(Channel::Nightly);
        let newest = publish(
            &bucket,
            Channel::Nightly,
            pin + 300,
            &release_of(script("n2")),
        );
        block(updater.check(&client()));
        assert!(matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == newest));
    }

    // ---- the boot step, on a Linux install --------------------------------------

    /// Stage `exe` as the release after the installed one, through a check.
    fn stage(layout: &Layout, bucket: &Bucket, sequence: u64, exe: Vec<u8>) -> Sha {
        let sha = publish(bucket, Channel::Stable, sequence, &release_of(exe));
        let mut updater = updater(layout, bucket, Channel::Stable);
        block(updater.check(&client()));
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
        let staged = Phase::Staged(app_update::Staged {
            current: a,
            previous: None,
            pinned_sequence: 10,
            staged: b,
            sequence: 11,
            display: "0.1.0+b".into(),
            refused: None,
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
            let swapping = Phase::Swapping(app_update::Swapping {
                from: a,
                to: b,
                pinned_sequence: pin + 1,
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
        let mut running = updater(&layout, &bucket, Channel::Stable);
        running.start();
        block(running.check(&client()));
        assert!(matches!(state(&layout), Phase::Staged(ref staged) if staged.staged == b));

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

        // A runs, reads the rollback, and the channel's B is not taken again
        let mut running =
            Updater::new(layout.clone(), phase, keys(), Channel::Stable, &bucket.base);
        running.start();
        assert_eq!(
            running.shown().status,
            format!("Went back from {}: it never came up", b.short())
        );
        block(running.check(&client()));
        assert_eq!(
            state(&layout),
            Phase::Idle(Idle {
                current: a,
                previous: Some(b),
                pinned_sequence: pin + 1
            })
        );
        let archive = layout::archive_name(&b, &Platform::HOST.key());
        assert_eq!(bucket.asked_for(&archive), 1, "B was downloaded once");
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

    use app_update::{Idle, Phase};

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
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>{APP_EXE}</string><key>CFBundleIdentifier</key><string>dev.ducktape.test.{name}</string></dict></plist>\n"
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
        let json = serde_json::to_vec_pretty(&manifest).unwrap();
        let signature = Signature::sign(&signing_key_from_hex(&seed()).unwrap(), &json).encoded();
        bucket.put(&layout::archive_name(&sha, &Platform::HOST.key()), bytes);
        bucket.put("stable.json", json);
        bucket.put("stable.json.sig", signature.into_bytes());
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
    }

    /// A newer bundle dragged over an installed one finds a state pinned
    /// below it, which no longer describes what runs: the start adopts the
    /// bundle, and a release the older state had staged is dropped, never
    /// swapped in over it.
    #[test]
    fn a_newer_bundle_dragged_over_an_install_is_not_taken_back() {
        let (root, layout) = rig();
        let download = root.path().join("download");
        std::fs::create_dir_all(&download).unwrap();
        let a =
            crate::update::install::install(&layout, &bundle_exe(&bundle(&download, "a"))).unwrap();
        let own = layout.app_exe();
        let sequence = crate::update::own_sequence();
        assert!(sequence > 100, "the test build knows its commit's time");
        let older = Sha::digest(b"older");
        bundle(&layout.release_dir(older), "older");
        let behind = Phase::Staged(app_update::Staged {
            current: a,
            previous: None,
            pinned_sequence: sequence - 100,
            staged: older,
            sequence: sequence - 50,
            display: "0.1.0+older".into(),
            refused: None,
        });
        crate::update::persist_phase(&layout, &behind).unwrap();
        assert_eq!(boot_at(&layout, &own).unwrap(), Boot::Run(adopted(&own)));
        assert!(
            !layout.release_dir(older).exists(),
            "the older release is dropped"
        );
        let output = Command::new(&own).arg("--version").output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "ducktape-app 0.0.0+a"
        );
    }

    /// The headline on macOS: install, stage a newer signed bundle, the next
    /// start swaps it in, it never renders, the start after swaps back.
    #[test]
    fn an_installed_bundle_runs_a_newer_signed_bundle_next_start_and_one_that_never_renders_is_swapped_back()
     {
        let bucket = Bucket::new();
        let (root, layout) = rig();
        let download = root.path().join("download");
        std::fs::create_dir_all(&download).unwrap();
        let own = bundle_exe(&bundle(&download, "a"));
        let a = crate::update::install::install(&layout, &own).unwrap();
        let installed = state(&layout);
        let pin = installed.pinned_sequence();
        let own_a = layout.app_exe();
        assert_eq!(boot_at(&layout, &own_a).unwrap(), Boot::Run(installed));

        let built = root.path().join("built");
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
        block(running.check(&client()));
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
        block(running.check(&client()));
        assert_eq!(
            state(&layout),
            Phase::Idle(Idle {
                current: a,
                previous: Some(b),
                pinned_sequence: pin + 1
            })
        );
        let archive = layout::archive_name(&b, &Platform::HOST.key());
        assert_eq!(bucket.asked_for(&archive), 1, "B was downloaded once");
        // the whole tree is the test's to remove
        run(Command::new("chmod").args(["-R", "u+w"]).arg(root.path()));
    }
}
