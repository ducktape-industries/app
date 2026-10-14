//! The running app's half of the machine. The boot step arms it
//! ([`arm`]); the first frame starts it ([`rendered`]): that frame is the
//! healthy signal that settles a flip, and from then on a thread of its own
//! checks the channel now, every [`CHECK_INTERVAL`], and at once when the
//! person picks the other channel in Settings ([`choose`]).
//!
//! What runs here:
//! - `Fetch`: the chosen channel's one file over HTTPS, its manifest and the
//!   signature over it (at most [`MAX_MANIFEST_BYTES`]), verified under the
//!   pinned key and its
//!   successor, and against the highest sequence the channel has shown this
//!   install ([`Updater::against_seen`]), answering `ManifestFetched`.
//!   Unreachable or unpublished is not a refusal: the check did not happen,
//!   and the next is an interval away.
//! - `Download`: the archive into `releases/<sha>.partial`, resuming from the
//!   file's length with an HTTP `Range`, sha256 running over every byte the
//!   file holds; a mismatch deletes the partial.
//! - `Verify`: extract into `releases/<sha>/` ([`super::stage`]).
//! - `Gc`, `Persist`, `PinSuccessor`, `Banner`: the local
//!   writes, what Settings › Updates shows ([`shown`]), and one desktop
//!   notice per staged release.
//!
//! The flip is the next start's: nothing here qualifies, flips or `exec`s.
//! "Restart to update" ([`restart`]) only quits into that next start.
//! Trust is the signature under the pinned key and the monotonic sequence;
//! what the bucket serves is untrusted bytes until `verify_manifest` says
//! otherwise.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use app_update::layout::Channel;
use app_update::{
    Command, Event, Phase, Platform, RELEASE_KEY, Refusal as ManifestRefusal, RolledBack, Sha,
    SignedManifest, SuccessorKey, TrustedKeys, UpdateBanner, VerifiedManifest, layout, step,
    verify_manifest,
};
use tracing::{debug, info, warn};

use super::{Layout, TARGET, stage};

/// How often the running app asks the channel.
pub(super) const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// A channel file larger than this is not one.
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
/// The tag the update notice stands under: a newer staged release replaces it.
const NOTICE_TAG: &str = "ducktape/update";

/// The updater the boot step armed, until the first frame starts it.
static ARMED: Mutex<Option<Updater>> = Mutex::new(None);
/// The armed install, for Settings' picks and the restart.
static HOME: OnceLock<Layout> = OnceLock::new();
/// The channel the person picked; `None` while the updater is off.
/// Settings writes it, the check loop follows it at its next turn.
static CHOSEN: Mutex<Option<Channel>> = Mutex::new(None);
/// Wakes the check loop: a pick is checked now, not an hour from now.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// What Settings reads; `None` while the updater is off.
static READING: Mutex<Option<Reading>> = Mutex::new(None);
/// How the shell hears the reading moved ([`on_moved`]).
static MOVED: OnceLock<fn()> = OnceLock::new();
/// "Restart to update" was pressed: one waiter, however many presses.
static RESTARTING: AtomicBool = AtomicBool::new(false);
/// The boot step found another process of this install running its updater.
static ELSEWHERE: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The updater's state as Settings reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reading {
    phase: Phase,
    /// The channel the last check asked.
    channel: Channel,
    banner: Option<UpdateBanner>,
    /// A rollback the boot step made, read at this start.
    rolled_back: Option<RolledBack>,
}

/// The boot step found this image is the installed release.
pub(super) fn arm(layout: Layout, phase: Phase) {
    let channel = read_channel(&layout);
    let keys = trusted_keys(&layout);
    *lock(&CHOSEN) = Some(channel);
    let _ = HOME.set(layout.clone());
    let mut updater = Updater::new(layout, phase, keys, channel, layout::BASE_URL);
    updater.published = true;
    updater.publish();
    *lock(&ARMED) = Some(updater);
}

/// The channel this device follows: `<updates>/channel`, stable when unset.
pub(super) fn read_channel(layout: &Layout) -> Channel {
    std::fs::read_to_string(layout.channel_path())
        .ok()
        .and_then(|name| Channel::of(name.trim()))
        .unwrap_or_default()
}

/// What [`read_channel`] reads back.
pub(super) fn keep_channel(layout: &Layout, channel: Channel) -> Result<(), super::Refusal> {
    super::persist(
        &layout.channel_path(),
        format!("{}\n", channel.name()).as_bytes(),
    )
}

/// The boot step found the install's updater running in another of its
/// processes: Settings says so.
pub(super) fn elsewhere() {
    ELSEWHERE.store(true, Ordering::SeqCst);
}

/// The shell says how it hears the reading moved.
pub(crate) fn on_moved(moved: fn()) {
    let _ = MOVED.set(moved);
}

/// A window's first frame: the healthy signal, then the checks. Only the
/// first call does anything, and nothing without an armed updater.
pub(crate) fn rendered() {
    #[cfg(test)]
    RENDERED_HERE.with(|count| count.set(count.get() + 1));
    let Some(mut updater) = lock(&ARMED).take() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("updater".into())
        .spawn(move || {
            updater.start();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("the updater runtime");
            runtime.block_on(async move {
                let client = client();
                loop {
                    if let Some(chosen) = *lock(&CHOSEN) {
                        updater.switch(chosen);
                    }
                    updater.check(&client).await;
                    tokio::select! {
                        () = tokio::time::sleep(CHECK_INTERVAL) => {}
                        () = WAKE.notified() => {}
                    }
                }
            });
        });
    if let Err(error) = spawned {
        warn!(target: TARGET, event = "app_update_disabled", %error, "no updater thread");
    }
}

/// Settings' pick: kept in `<updates>/channel`, and checked now. Nothing
/// while the updater is off.
pub(crate) fn choose(channel: Channel) {
    // a test's page: the pick moves what it shows, as `CHOSEN` does
    #[cfg(test)]
    if SHOWN_HERE.with(|here| match here.borrow_mut().as_mut() {
        Some(Updates::Here(shown)) => {
            shown.channel = channel;
            true
        }
        _ => false,
    }) {
        return;
    }
    let Some(layout) = HOME.get() else {
        return;
    };
    if let Err(refusal) = keep_channel(layout, channel) {
        warn!(target: TARGET, event = "app_update_persist_failed", reason = refusal.reason, detail = %refusal.detail);
        return;
    }
    info!(target: TARGET, event = "app_update_channel", channel = channel.name());
    *lock(&CHOSEN) = Some(channel);
    WAKE.notify_one();
    moved();
}

/// "Restart to update": this app quits (the caller's part) and the install
/// path's app starts once this process is gone, so the next start's boot
/// step flips to the staged release with nothing of this one still running.
/// A waiter holds the read end of a pipe whose write end only this process
/// has: the kernel closes it as this process exits, and the waiter's `read`
/// returns. Linux runs `current/ducktape-app`; macOS opens the installed
/// bundle through LaunchServices.
pub(crate) fn restart() -> Result<(), String> {
    let layout = HOME.get().ok_or("the updater is off")?;
    if RESTARTING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let mut start = match cfg!(target_os = "macos") {
        true => vec!["/usr/bin/open".into(), layout.installed_bundle()],
        false => vec![layout.app_exe()],
    };
    let program = start.remove(0);
    let waiter = Process::new("/bin/sh")
        .args(["-c", "read _; exec \"$@\" </dev/null", "sh"])
        .arg(program)
        .args(start)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    match waiter {
        Ok(mut waiter) => {
            // the write end lives exactly as long as this process
            std::mem::forget(waiter.stdin.take());
            info!(target: TARGET, event = "app_update_restart");
            Ok(())
        }
        Err(error) => {
            RESTARTING.store(false, Ordering::SeqCst);
            warn!(target: TARGET, event = "app_update_restart_failed", %error);
            Err(error.to_string())
        }
    }
}

/// connect fast, never hang on a stalled read; no total cap, an archive
/// takes as long as the line does.
pub(super) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("the HTTP client")
}

/// The async work one step asks for; each answers with one [`Event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Job {
    Fetch,
    Download { sha: Sha, size: u64 },
    Verify(Sha),
}

/// The machine plus its files, driven by the check loop.
pub(super) struct Updater {
    layout: Layout,
    phase: Phase,
    keys: TrustedKeys,
    /// The channel the checks ask.
    channel: Channel,
    /// The channel's base URL: [`layout::BASE_URL`], or a test's server.
    base: String,
    banner: Option<UpdateBanner>,
    rolled_back: Option<RolledBack>,
    /// The highest sequence each channel's verified manifest has carried,
    /// by channel name: `<updates>/seen.json`.
    seen: BTreeMap<String, u64>,
    /// The process's own updater ([`arm`]): it, and no test's, is what
    /// Settings reads and what raises the desktop notice.
    published: bool,
}

/// The pinned key, and the successor a verified manifest announced.
fn trusted_keys(layout: &Layout) -> TrustedKeys {
    let successor = std::fs::read_to_string(layout.successor_path())
        .ok()
        .and_then(|text| {
            let parsed = serde_json::from_str::<SuccessorKey>(&text);
            if let Err(error) = &parsed {
                // the rotation is forgotten: the pinned key alone is trusted
                warn!(target: TARGET, event = "app_update_successor_unreadable", %error);
            }
            parsed.ok()
        });
    TrustedKeys {
        pinned: RELEASE_KEY,
        successor,
    }
}

impl Updater {
    pub(super) fn new(
        layout: Layout,
        phase: Phase,
        keys: TrustedKeys,
        channel: Channel,
        base: &str,
    ) -> Self {
        let seen = match std::fs::read_to_string(layout.seen_path()) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                // the marks are forgotten: the pin alone guards until the
                // next manifest sets them again
                warn!(target: TARGET, event = "app_update_seen_unreadable", %error);
                BTreeMap::new()
            }),
            Err(_) => BTreeMap::new(),
        };
        Updater {
            layout,
            phase,
            keys,
            channel,
            base: base.to_string(),
            banner: None,
            rolled_back: None,
            seen,
            published: false,
        }
    }

    #[cfg(test)]
    pub(super) fn phase(&self) -> &Phase {
        &self.phase
    }

    /// The first frame: a flip that came up settles, and a rollback the
    /// boot step made is read (logged, and kept for Settings this session).
    pub(super) fn start(&mut self) {
        let was_pending = matches!(self.phase, Phase::PendingHealthy(_));
        self.apply(Event::Rendered);
        if was_pending && matches!(self.phase, Phase::Idle(_)) {
            info!(target: TARGET, event = "app_update_settled", release = %self.phase.current(), pinned_sequence = self.phase.pinned_sequence());
        }
        if let Phase::RolledBack(rolled_back) = &self.phase {
            warn!(target: TARGET, event = "app_update_rolled_back", failed = %rolled_back.failed, current = %rolled_back.current, reason = %rolled_back.reason);
            self.rolled_back = Some(rolled_back.clone());
            self.apply(Event::DismissRollbackNotice);
        }
    }

    /// Follow `channel` from the next check on: a release the old one
    /// offered is dropped, and its last word with it.
    pub(super) fn switch(&mut self, channel: Channel) {
        if channel == self.channel {
            return;
        }
        self.channel = channel;
        self.banner = None;
        self.apply(Event::ChannelChanged);
    }

    /// One check: `Tick`, then every job it leads to, in turn.
    pub(super) async fn check(&mut self, client: &reqwest::Client) {
        let mut job = self.apply(Event::Tick);
        while let Some(next) = job.take() {
            let Some(event) = self.run(client, next).await else {
                return;
            };
            job = self.apply(event);
        }
    }

    /// Feed one event through `step` and perform its local commands.
    /// Returns the job to run next, if the commands asked for one.
    pub(super) fn apply(&mut self, event: Event) -> Option<Job> {
        let (phase, commands) = step(self.phase.clone(), event);
        let mut job = None;
        for command in commands {
            match command {
                Command::Persist(written) => {
                    if let Err(refusal) = super::persist_phase(&self.layout, &written) {
                        warn!(target: TARGET, event = "app_update_persist_failed", reason = refusal.reason, detail = %refusal.detail);
                    }
                }
                Command::Fetch => job = Some(Job::Fetch),
                Command::Download { sha, size } => job = Some(Job::Download { sha, size }),
                Command::Verify(sha) => job = Some(Job::Verify(sha)),
                Command::PinSuccessor(successor) => {
                    let text = serde_json::to_string_pretty(&successor).expect("a key serializes");
                    if let Err(refusal) =
                        super::persist(&self.layout.successor_path(), text.as_bytes())
                    {
                        warn!(target: TARGET, event = "app_update_persist_failed", reason = refusal.reason, detail = %refusal.detail);
                    }
                    self.keys.successor = Some(successor);
                }
                Command::Banner(banner) => {
                    note_banner(&banner);
                    if let (true, UpdateBanner::Ready { display, .. }) = (self.published, &banner) {
                        notify_ready(display);
                    }
                    self.banner = Some(banner);
                }
                Command::Gc { keep } => {
                    stage::collect(&self.layout.releases_dir(), &keep);
                    // `previous` naming a release that went goes with it
                    let previous = self.layout.previous_link();
                    let named = super::read_link(&previous).ok().flatten();
                    let gone = named.is_some_and(|target| {
                        !keep.iter().any(|sha| target == Layout::link_target(*sha))
                    });
                    if gone && let Err(refusal) = super::remove_leftover(&previous) {
                        warn!(target: TARGET, event = "app_update_gc_failed", reason = refusal.reason, detail = %refusal.detail);
                    }
                }
                // the boot step's; no event the running app raises leads here
                command @ (Command::Qualify(_)
                | Command::ResolveSwap { .. }
                | Command::Flip { .. }
                | Command::Exec(_)) => {
                    warn!(target: TARGET, event = "app_update_ignored", ?command, "a boot step's command");
                }
            }
        }
        self.phase = phase;
        self.publish();
        job
    }

    /// One job; `None` is a check that did not happen (unreachable, nothing
    /// published), which is not a refusal.
    async fn run(&mut self, client: &reqwest::Client, job: Job) -> Option<Event> {
        match job {
            Job::Fetch => {
                let fetched = fetch(client, &self.base, self.channel, &self.keys).await?;
                Some(Event::ManifestFetched(
                    fetched.and_then(|verified| self.against_seen(verified)),
                ))
            }
            Job::Download { sha, size } => Some(
                match download(client, &self.base, &self.layout, sha, size).await {
                    Ok(()) => Event::DownloadFinished { sha },
                    Err(reason) => Event::DownloadFailed { sha, reason },
                },
            ),
            Job::Verify(sha) => Some(verify(&self.layout, sha)),
        }
    }

    /// Replay: whoever serves the bucket can serve an older signed manifest
    /// that is still newer than what runs. The highest sequence each
    /// channel's verified manifest has carried is kept beside the update
    /// state, written before anything it offers is downloaded and never
    /// lowered (a switch, a rollback and an install keep it); a manifest
    /// below its channel's mark is refused.
    fn against_seen(
        &mut self,
        verified: VerifiedManifest,
    ) -> Result<VerifiedManifest, ManifestRefusal> {
        let (channel, sequence) = (self.channel.name(), verified.manifest.sequence);
        let seen = self.seen.get(channel).copied().unwrap_or(0);
        if sequence < seen {
            return Err(ManifestRefusal::SequenceReplayed);
        }
        if sequence > seen {
            self.seen.insert(channel.to_string(), sequence);
            let text = serde_json::to_string_pretty(&self.seen).expect("the marks serialize");
            if let Err(refusal) = super::persist(&self.layout.seen_path(), text.as_bytes()) {
                warn!(target: TARGET, event = "app_update_persist_failed", reason = refusal.reason, detail = %refusal.detail);
            }
        }
        Ok(verified)
    }

    fn reading(&self) -> Reading {
        Reading {
            phase: self.phase.clone(),
            channel: self.channel,
            banner: self.banner.clone(),
            rolled_back: self.rolled_back.clone(),
        }
    }

    /// What Settings would show of this updater.
    #[cfg(test)]
    pub(super) fn shown(&self) -> Shown {
        Shown::of(self.channel, &self.reading())
    }

    fn publish(&self) {
        if self.published {
            *lock(&READING) = Some(self.reading());
            moved();
        }
    }
}

fn moved() {
    if let Some(moved) = MOVED.get() {
        moved();
    }
}

/// The desktop notice for a staged release: one per release, the next
/// replacing it. The device's banners switch decides; a click opens
/// Settings › Updates.
fn notify_ready(display: &str) {
    let notice = crate::runtime::notify::Notice {
        title: format!("Ducktape {display} is ready."),
        body: "Restart to update.".into(),
        tag: NOTICE_TAG.into(),
    };
    let raised = crate::runtime::notify::post_host(&notice);
    debug!(target: TARGET, event = "app_update_notice", raised);
}

/// `Fetch`: `channel`'s file, its manifest verified.
pub(super) async fn fetch(
    client: &reqwest::Client,
    base: &str,
    channel: Channel,
    keys: &TrustedKeys,
) -> Option<Result<VerifiedManifest, ManifestRefusal>> {
    let file = match get_small(client, &format!("{base}{}", channel.file())).await {
        Ok(file) => file,
        Err(error) => {
            debug!(target: TARGET, event = "app_update_manifest_unavailable", %error);
            return None;
        }
    };
    Some(
        SignedManifest::from_file(&file)
            .and_then(|signed| verify_manifest(&signed, channel.name(), keys)),
    )
}

/// A whole small file, refused past [`MAX_MANIFEST_BYTES`] whatever the
/// server claims its length is.
async fn get_small(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(format!("{url} is larger than a manifest"));
        }
    }
    Ok(bytes)
}

/// `Download`: the archive into `releases/<sha>.partial`, resuming from what
/// is already there, sha256 running over every byte the file holds. `Ok`
/// means the file is complete and hashes to `sha`; an error is a stable
/// token, and only bytes proven wrong are deleted.
pub(super) async fn download(
    client: &reqwest::Client,
    base: &str,
    layout: &Layout,
    sha: Sha,
    size: u64,
) -> Result<(), String> {
    use sha2::Digest as _;
    let path = layout.partial_path(sha);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "io")?;
    }
    super::refuse_symlink(&path).map_err(|refusal| refusal.reason)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .map_err(|_| "io")?;
    let mut have = file.metadata().map_err(|_| "io")?.len();
    if have > size {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err("partial_overlong".into());
    }
    // resume: what is on disk is hashed first, streamed
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|_| "io")?;
    if have < size {
        let url = format!(
            "{base}{}",
            layout::archive_name(&sha, &Platform::HOST.key())
        );
        let mut request = client.get(url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request.send().await.map_err(|_| "unreachable")?;
        match response.status().as_u16() {
            206 if have > 0 => {}
            // the whole file: a server that ignored the range starts over
            200 => {
                file.set_len(0).map_err(|_| "io")?;
                hasher = sha2::Sha256::new();
                have = 0;
            }
            status => return Err(format!("http_{status}")),
        }
        debug!(target: TARGET, event = "app_update_download", %sha, have, size);
        while let Some(chunk) = response.chunk().await.map_err(|_| "read_failed")? {
            have += chunk.len() as u64;
            if have > size {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err("overlong".into());
            }
            file.write_all(&chunk).map_err(|_| "io")?;
            hasher.update(&chunk);
        }
        if have < size {
            return Err("short_file".into());
        }
    }
    file.sync_all().map_err(|_| "io")?;
    drop(file);
    if Sha::from_bytes(hasher.finalize().into()) != sha {
        let _ = std::fs::remove_file(&path);
        warn!(target: TARGET, event = "app_update_refused", stage = "download", %sha, reason = "sha256_mismatch");
        return Err("sha256_mismatch".into());
    }
    info!(target: TARGET, event = "app_update_downloaded", %sha, size);
    Ok(())
}

/// `Verify`: the partial into `releases/<sha>/`, checked. A refusal removes
/// what was extracted; the partial stays unless its bytes were the problem,
/// so the next offer of the same release costs no second download.
pub(super) fn verify(layout: &Layout, sha: Sha) -> Event {
    let release_dir = layout.release_dir(sha);
    match stage::stage(&layout.partial_path(sha), sha, &release_dir) {
        Ok(()) => {
            info!(target: TARGET, event = "app_update_verified", %sha);
            Event::Verified(sha)
        }
        Err(reason) => {
            let _ = super::remove_leftover(&release_dir);
            if reason == "sha256_mismatch" {
                let _ = std::fs::remove_file(layout.partial_path(sha));
            }
            Event::VerifyRefused { sha, reason }
        }
    }
}

/// The channel's last word, in the log.
pub(super) fn note_banner(banner: &UpdateBanner) {
    match banner {
        UpdateBanner::Ready {
            staged,
            display: release,
        } => {
            info!(target: TARGET, event = "app_update_staged", %staged, %release)
        }
        UpdateBanner::UpToDate => debug!(target: TARGET, event = "app_update_up_to_date"),
        // the channel's release is older than what runs: after a switch,
        // the wait until it passes; otherwise a replay, refused all the same
        UpdateBanner::Refused(ManifestRefusal::SequenceNotNewer) => {
            info!(target: TARGET, event = "app_update_behind")
        }
        UpdateBanner::Refused(refusal) => {
            warn!(target: TARGET, event = "app_update_refused", reason = %refusal)
        }
        UpdateBanner::DownloadFailed { target, reason } => {
            warn!(target: TARGET, event = "app_update_refused", stage = "download", sha = %target, %reason)
        }
        UpdateBanner::VerifyRefused { target, reason } => {
            warn!(target: TARGET, event = "app_update_refused", stage = "verify", sha = %target, %reason)
        }
        UpdateBanner::QualifyFailed { staged, reason } => {
            warn!(target: TARGET, event = "app_update_refused", stage = "qualify", sha = %staged, %reason)
        }
    }
}

/// Where this process stands, for Settings › Updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Updates {
    /// Not an installed copy: nothing here updates.
    Off,
    /// An installed copy whose updater another of its processes runs.
    Elsewhere,
    /// This process is the install's updater.
    Here(Shown),
}

/// What Settings › Updates shows of the updater this process runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shown {
    /// The channel the person picked.
    pub(crate) channel: Channel,
    /// Where the updater is, in words.
    pub(crate) status: String,
    /// A staged release waits for the next start: "Restart to update".
    pub(crate) restart: bool,
}

#[cfg(test)]
thread_local! {
    static SHOWN_HERE: std::cell::RefCell<Option<Updates>> = const { std::cell::RefCell::new(None) };
    static RENDERED_HERE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times [`rendered`] was called on this test's thread.
#[cfg(test)]
pub(crate) fn rendered_here() -> usize {
    RENDERED_HERE.with(std::cell::Cell::get)
}

/// What Settings › Updates shows on this test's thread, in place of the
/// process's (off in every test).
#[cfg(test)]
pub(crate) fn show_here(updates: Option<Updates>) {
    SHOWN_HERE.with(|here| *here.borrow_mut() = updates);
}

/// Settings › Updates.
pub(crate) fn updates() -> Updates {
    #[cfg(test)]
    if let Some(updates) = SHOWN_HERE.with(|here| here.borrow().clone()) {
        return updates;
    }
    match (*lock(&CHOSEN), lock(&READING).clone()) {
        (Some(chosen), Some(reading)) => Updates::Here(Shown::of(chosen, &reading)),
        _ if ELSEWHERE.load(Ordering::SeqCst) => Updates::Elsewhere,
        _ => Updates::Off,
    }
}

impl Shown {
    fn of(chosen: Channel, reading: &Reading) -> Shown {
        let restart = matches!(&reading.phase, Phase::Staged(staged) if staged.refused.is_none());
        Shown {
            channel: chosen,
            status: status(chosen, reading),
            restart,
        }
    }
}

fn title(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "Stable",
        Channel::Nightly => "Nightly",
    }
}

fn status(chosen: Channel, reading: &Reading) -> String {
    match &reading.phase {
        Phase::Staged(staged) => {
            return match &staged.refused {
                None => format!(
                    "{} is ready: it takes over at the next start",
                    staged.display
                ),
                Some(reason) => {
                    format!(
                        "{} did not start ({reason}): this one keeps running",
                        staged.display
                    )
                }
            };
        }
        Phase::Downloading(downloading) => return format!("Downloading {}", downloading.display),
        _ => {}
    }
    if let Some(rolled_back) = &reading.rolled_back {
        return format!(
            "Went back from {}: it never came up",
            rolled_back.failed.short()
        );
    }
    // a pick the loop has not checked yet
    if chosen != reading.channel {
        return format!("Checking {}…", title(chosen));
    }
    match &reading.banner {
        None => "Not checked yet".into(),
        Some(UpdateBanner::UpToDate | UpdateBanner::Ready { .. }) => "Up to date".into(),
        Some(UpdateBanner::Refused(ManifestRefusal::SequenceNotNewer)) => format!(
            "{} has nothing newer than this version yet: this one stays until it does",
            title(reading.channel)
        ),
        Some(UpdateBanner::Refused(refusal)) => format!("Last check refused: {refusal}"),
        Some(UpdateBanner::DownloadFailed { reason, .. }) => format!("Download failed: {reason}"),
        Some(UpdateBanner::VerifyRefused { reason, .. }) => {
            format!("Verification refused: {reason}")
        }
        Some(UpdateBanner::QualifyFailed { reason, .. }) => {
            format!("The new release did not start: {reason}")
        }
    }
}
