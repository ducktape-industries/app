//! The running app's half of the machine. The boot step arms it
//! ([`arm`]); the first frame starts it ([`rendered`]): that frame is the
//! healthy signal that settles a flip, and from then on a thread of its own
//! checks the channel now, every [`CHECK_INTERVAL`], and at once when the
//! person picks the other channel in Settings ([`choose`]), flips "Update
//! automatically" ([`automate`]), presses "Update now" ([`update_now`]) or
//! "Check now" ([`check_now`]).
//! With updating automatically off a check only names a newer release
//! (`Take::Nothing`), and a release the updater took on its own before the
//! switch went off is dropped (`AutomaticOff`); "Update now" is one check
//! that takes, as asked (`Take::Asked`).
//!
//! What runs here:
//! - `Fetch`: the chosen channel's one file over HTTPS, its manifest and the
//!   signature over it, verified under the pinned key and its
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
//!   notice per staged release, with its row in the notification centre;
//!   the first frame after a flip says what it installed, the same way.
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

use ducktape_updater::layout::Channel;
use ducktape_updater::{
    Command, Event, Phase, Platform, RELEASE_KEY, Refusal as ManifestRefusal, RolledBack, Sha,
    SignedManifest, SuccessorKey, Take, TrustedKeys, UpdateBanner, VerifiedManifest, layout, step,
    verify_manifest,
};
use tracing::{debug, info, warn};

use super::{Layout, TARGET, stage};
use crate::runtime::notify::{CenterHandle, HOST};

pub(super) const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const READY: &str = "update/ready";
const INSTALLED: &str = "update/installed";

static ARMED: Mutex<Option<Updater>> = Mutex::new(None);
static HOME: OnceLock<Layout> = OnceLock::new();
static CHOSEN: Mutex<Option<Channel>> = Mutex::new(None);
static AUTOMATIC: AtomicBool = AtomicBool::new(true);
static ASKED: AtomicBool = AtomicBool::new(false);
static CHECKING: AtomicBool = AtomicBool::new(false);
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
static READING: Mutex<Option<Reading>> = Mutex::new(None);
static MOVED: OnceLock<fn()> = OnceLock::new();
static RESTARTING: AtomicBool = AtomicBool::new(false);
static ELSEWHERE: AtomicBool = AtomicBool::new(false);
static RENDERED: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reading {
    phase: Phase,
    channel: Channel,
    banner: Option<UpdateBanner>,
    rolled_back: Option<RolledBack>,
    checked: Option<Checked>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Checked {
    at: i64,
    answered: bool,
}

pub(super) fn arm(layout: Layout, phase: Phase) {
    let channel = read_channel(&layout);
    let keys = trusted_keys(&layout);
    *lock(&CHOSEN) = Some(channel);
    AUTOMATIC.store(read_automatic(&layout), Ordering::SeqCst);
    let _ = HOME.set(layout.clone());
    let mut updater = Updater::new(layout, phase, keys, channel, layout::BASE_URL);
    updater.published = true;
    updater.center = crate::runtime::notify::center().clone();
    updater.publish();
    *lock(&ARMED) = Some(updater);
}

pub(super) fn read_channel(layout: &Layout) -> Channel {
    std::fs::read_to_string(layout.channel_path())
        .ok()
        .and_then(|name| Channel::of(name.trim()))
        .unwrap_or_default()
}

pub(super) fn keep_channel(layout: &Layout, channel: Channel) -> Result<(), super::Refusal> {
    super::persist(
        &layout.channel_path(),
        format!("{}\n", channel.name()).as_bytes(),
    )
}

pub(super) fn read_automatic(layout: &Layout) -> bool {
    !std::fs::read_to_string(layout.automatic_path()).is_ok_and(|said| said.trim() == "off")
}

pub(super) fn keep_automatic(layout: &Layout, automatic: bool) -> Result<(), super::Refusal> {
    let said = match automatic {
        true => "on\n",
        false => "off\n",
    };
    super::persist(&layout.automatic_path(), said.as_bytes())
}

pub(super) fn elsewhere() {
    ELSEWHERE.store(true, Ordering::SeqCst);
}

pub(super) fn took_over(layout: Layout, phase: Phase) {
    arm(layout, phase);
    ELSEWHERE.store(false, Ordering::SeqCst);
    moved();
    if RENDERED.load(Ordering::SeqCst) {
        start_armed();
    }
}

pub(crate) fn on_moved(moved: fn()) {
    let _ = MOVED.set(moved);
}

pub(crate) fn rendered() {
    #[cfg(test)]
    RENDERED_HERE.with(|count| count.set(count.get() + 1));
    RENDERED.store(true, Ordering::SeqCst);
    start_armed();
}

fn start_armed() {
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
                    let automatic = AUTOMATIC.load(Ordering::SeqCst);
                    if !automatic {
                        updater.apply(Event::AutomaticOff);
                    }
                    let take = match (ASKED.swap(false, Ordering::SeqCst), automatic) {
                        (true, _) => Take::Asked,
                        (false, true) => Take::Automatic,
                        (false, false) => Take::Nothing,
                    };
                    CHECKING.store(true, Ordering::SeqCst);
                    moved();
                    updater.check(&client, take).await;
                    CHECKING.store(false, Ordering::SeqCst);
                    moved();
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

pub(crate) fn choose(channel: Channel) {
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

pub(crate) fn automate(automatic: bool) {
    let Some(layout) = HOME.get() else {
        return;
    };
    if let Err(refusal) = keep_automatic(layout, automatic) {
        warn!(target: TARGET, event = "app_update_persist_failed", reason = refusal.reason, detail = %refusal.detail);
        return;
    }
    info!(target: TARGET, event = "app_update_automatic", automatic);
    AUTOMATIC.store(automatic, Ordering::SeqCst);
    WAKE.notify_one();
    moved();
}

pub(crate) fn update_now() {
    if HOME.get().is_none() {
        return;
    }
    info!(target: TARGET, event = "app_update_asked");
    ASKED.store(true, Ordering::SeqCst);
    WAKE.notify_one();
}

pub(crate) fn check_now() {
    if HOME.get().is_none() || CHECKING.swap(true, Ordering::SeqCst) {
        return;
    }
    info!(target: TARGET, event = "app_update_check_now");
    WAKE.notify_one();
    moved();
}

pub(crate) fn restart() -> Result<(), String> {
    HOME.get().ok_or("the updater is off")?;
    let own = std::env::current_exe().map_err(|error| error.to_string())?;
    if RESTARTING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let waiter = Process::new(own)
        .arg(RESTART_WAITER)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    match waiter {
        Ok(mut waiter) => {
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

pub(crate) const RESTART_WAITER: &str = "--restart-waiter";

pub(crate) fn restart_waiter() -> i32 {
    use std::io::Read as _;
    let _ = std::io::stdin().read_to_end(&mut Vec::new());
    let Some(layout) = Layout::host() else {
        warn!(target: TARGET, event = "app_update_restart_failed", reason = "layout_unresolvable");
        return 1;
    };
    if let Err(refusal) = super::await_install(&layout) {
        warn!(target: TARGET, event = "app_update_restart_failed", reason = refusal.reason, detail = %refusal.detail);
        return 1;
    }
    if cfg!(target_os = "macos") {
        return match super::macos::open_new(&layout.installed_bundle(), &[]) {
            Ok(()) => {
                info!(target: TARGET, event = "app_update_restart_opened");
                0
            }
            Err(said) => {
                warn!(target: TARGET, event = "app_update_restart_failed", reason = "open_failed", said);
                1
            }
        };
    }
    info!(target: TARGET, event = "app_update_restart_exec", exe = %layout.app_exe().display());
    let error = Process::new(layout.app_exe()).stdin(Stdio::null()).exec();
    warn!(target: TARGET, event = "app_update_restart_failed", reason = "exec_failed", %error);
    1
}

pub(super) fn client() -> reqwest::Client {
    reqwest::Client::builder().build().expect("the HTTP client")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Job {
    Fetch,
    Download { sha: Sha, size: u64 },
    Verify(Sha),
}

pub(super) struct Updater {
    layout: Layout,
    phase: Phase,
    keys: TrustedKeys,
    channel: Channel,
    base: String,
    banner: Option<UpdateBanner>,
    rolled_back: Option<RolledBack>,
    seen: BTreeMap<String, u64>,
    checked: Option<Checked>,
    published: bool,
    center: CenterHandle,
}

fn trusted_keys(layout: &Layout) -> TrustedKeys {
    let successor = std::fs::read_to_string(layout.successor_path())
        .ok()
        .and_then(|text| {
            let parsed = serde_json::from_str::<SuccessorKey>(&text);
            if let Err(error) = &parsed {
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
            checked: None,
            seen,
            published: false,
            center: CenterHandle::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn phase(&self) -> &Phase {
        &self.phase
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn with_center(mut self, center: CenterHandle) -> Self {
        self.center = center;
        self
    }

    pub(super) fn start(&mut self) {
        let was_pending = matches!(self.phase, Phase::PendingHealthy(_));
        self.apply(Event::Rendered);
        let settled = was_pending && matches!(self.phase, Phase::Idle(_));
        if settled {
            info!(target: TARGET, event = "app_update_settled", release = %self.phase.current(), pinned_sequence = self.phase.pinned_sequence());
        }
        if let Phase::RolledBack(rolled_back) = &self.phase {
            warn!(target: TARGET, event = "app_update_rolled_back", failed = %rolled_back.failed, current = %rolled_back.current, reason = %rolled_back.reason);
            self.rolled_back = Some(rolled_back.clone());
            self.apply(Event::DismissRollbackNotice);
        }
        match self.phase {
            _ if settled => self.notice(Said::Installed(&super::version())),
            Phase::Staged(_) => {}
            _ => self.notice(Said::Withdrawn),
        }
    }

    pub(super) fn switch(&mut self, channel: Channel) {
        if channel == self.channel {
            return;
        }
        self.channel = channel;
        self.banner = None;
        self.checked = None;
        self.apply(Event::ChannelChanged);
    }

    pub(super) async fn check(&mut self, client: &reqwest::Client, take: Take) {
        if let Some(answered) = self.jobs(client, take).await {
            self.checked = Some(Checked {
                at: crate::runtime::notify::wall(),
                answered,
            });
            self.publish();
        }
    }

    async fn jobs(&mut self, client: &reqwest::Client, take: Take) -> Option<bool> {
        let asked = self.channel;
        let mut answered = None;
        let mut job = self.apply(Event::Tick);
        while let Some(next) = job.take() {
            let fetch = next == Job::Fetch;
            let event = self.run(client, next, take).await;
            if fetch {
                answered = Some(event.is_some());
            }
            let Some(event) = event else {
                return answered;
            };
            let now = read_channel(&self.layout);
            if now != asked {
                info!(target: TARGET, event = "app_update_dropped", asked = asked.name(), now = now.name(), "the channel moved while this check ran");
                return None;
            }
            if take == Take::Automatic && !read_automatic(&self.layout) {
                info!(target: TARGET, event = "app_update_dropped", "updating automatically went off while this check ran");
                return answered;
            }
            job = self.apply(event);
        }
        answered
    }

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
                    if let UpdateBanner::Ready { display, .. } = &banner {
                        self.notice(Said::Ready(display));
                    }
                    self.banner = Some(banner);
                }
                Command::Gc { keep } => super::collect(&self.layout, &keep),
                command @ (Command::Qualify(_)
                | Command::ResolveSwap { .. }
                | Command::Flip { .. }
                | Command::Exec(_)) => {
                    warn!(target: TARGET, event = "app_update_ignored", ?command, "a boot step's command");
                }
            }
        }
        let unstaged = matches!(self.phase, Phase::Staged(_)) && !matches!(phase, Phase::Staged(_));
        self.phase = phase;
        if unstaged {
            self.notice(Said::Withdrawn);
        }
        self.publish();
        job
    }

    fn notice(&self, said: Said) {
        #[cfg(test)]
        NOTICED_HERE.with(|noticed| {
            noticed.borrow_mut().push(match said {
                Said::Ready(display) => format!("ready {display}"),
                Said::Installed(version) => format!("installed {version}"),
                Said::Withdrawn => "withdrawn".into(),
            });
        });
        let (tag, title, body) = match said {
            Said::Ready(display) => (
                READY,
                format!("Ducktape {display} is ready."),
                "Restart to update.",
            ),
            Said::Installed(version) => {
                self.withdraw();
                (
                    INSTALLED,
                    format!("Ducktape {version} is installed."),
                    "It runs from now on.",
                )
            }
            Said::Withdrawn => return self.withdraw(),
        };
        let wall = crate::runtime::notify::wall();
        self.center.lock().post_host(&title, body, tag, wall);
        if self.published {
            raise(tag, title, body);
            moved();
        }
    }

    fn withdraw(&self) {
        self.center.lock().withdraw_host(READY);
        if self.published {
            crate::os::withdraw_host();
            debug!(target: TARGET, event = "app_update_notice_withdrawn");
            moved();
        }
    }

    async fn run(&mut self, client: &reqwest::Client, job: Job, take: Take) -> Option<Event> {
        match job {
            Job::Fetch => {
                let fetched = fetch(client, &self.base, self.channel, &self.keys).await?;
                Some(Event::ManifestFetched {
                    result: fetched.and_then(|verified| self.against_seen(verified)),
                    take,
                })
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
            checked: self.checked,
        }
    }

    #[cfg(test)]
    pub(super) fn shown(&self) -> Shown {
        Shown::of(self.channel, read_automatic(&self.layout), &self.reading())
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

#[derive(Clone, Copy)]
enum Said<'a> {
    Ready(&'a str),
    Installed(&'a str),
    Withdrawn,
}

fn raise(tag: &str, title: String, body: &str) {
    let notice = crate::runtime::notify::Notice {
        title,
        body: body.into(),
        tag: format!("{HOST}/{tag}"),
        ..Default::default()
    };
    let raised = crate::os::post_host(&notice);
    debug!(target: TARGET, event = "app_update_notice", raised);
}

pub(super) async fn fetch(
    client: &reqwest::Client,
    base: &str,
    channel: Channel,
    keys: &TrustedKeys,
) -> Option<Result<VerifiedManifest, ManifestRefusal>> {
    let file = match get_whole(client, &format!("{base}{}", channel.file())).await {
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

async fn get_whole(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    Ok(bytes.to_vec())
}

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

pub(super) fn verify(layout: &Layout, sha: Sha) -> Event {
    let release_dir = layout.release_dir(sha);
    let staged = stage::stage(&layout.partial_path(sha), sha, &release_dir).and_then(|()| {
        match cfg!(target_os = "macos") {
            true => super::macos::record_executable(layout, sha)
                .map_err(|refusal| refusal.reason.to_string()),
            false => Ok(()),
        }
    });
    match staged {
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

pub(super) fn note_banner(banner: &UpdateBanner) {
    match banner {
        UpdateBanner::Ready {
            staged,
            display: release,
        } => {
            info!(target: TARGET, event = "app_update_staged", %staged, %release)
        }
        UpdateBanner::Available {
            offered,
            display: release,
        } => info!(target: TARGET, event = "app_update_available", %offered, %release),
        UpdateBanner::UpToDate => debug!(target: TARGET, event = "app_update_up_to_date"),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Updates {
    Off,
    Elsewhere,
    Here(Shown),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shown {
    pub(crate) channel: Channel,
    pub(crate) automatic: bool,
    pub(crate) status: String,
    pub(crate) restart: bool,
    pub(crate) update_now: bool,
    pub(crate) checked: Option<String>,
    pub(crate) checking: bool,
}

#[cfg(test)]
thread_local! {
    static SHOWN_HERE: std::cell::RefCell<Option<Updates>> = const { std::cell::RefCell::new(None) };
    static RENDERED_HERE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static NOTICED_HERE: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) fn noticed_here() -> Vec<String> {
    NOTICED_HERE.with(|noticed| std::mem::take(&mut *noticed.borrow_mut()))
}

#[cfg(test)]
pub(crate) fn rendered_here() -> usize {
    RENDERED_HERE.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn show_here(updates: Option<Updates>) {
    SHOWN_HERE.with(|here| *here.borrow_mut() = updates);
}

pub(crate) fn updates() -> Updates {
    #[cfg(test)]
    if let Some(updates) = SHOWN_HERE.with(|here| here.borrow().clone()) {
        return updates;
    }
    match (*lock(&CHOSEN), lock(&READING).clone()) {
        (Some(chosen), Some(reading)) => Updates::Here(Shown::of(
            chosen,
            AUTOMATIC.load(Ordering::SeqCst),
            &reading,
        )),
        _ if ELSEWHERE.load(Ordering::SeqCst) => Updates::Elsewhere,
        _ => Updates::Off,
    }
}

impl Shown {
    fn of(chosen: Channel, automatic: bool, reading: &Reading) -> Shown {
        let restart = matches!(&reading.phase, Phase::Staged(staged) if staged.refused.is_none() && (automatic || staged.asked));
        let update_now = !automatic
            && matches!(
                (&reading.phase, &reading.banner),
                (Phase::Idle(_), Some(UpdateBanner::Available { .. }))
            );
        Shown {
            channel: chosen,
            automatic,
            status: status(chosen, reading),
            restart,
            update_now,
            checked: last_check(chosen, reading),
            checking: CHECKING.load(Ordering::SeqCst),
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
        let failed = match rolled_back.display.is_empty() {
            true => rolled_back.failed.short(),
            false => rolled_back.display.clone(),
        };
        return format!("Went back from {failed}: it never came up");
    }
    if chosen != reading.channel {
        return format!("Checking {}…", title(chosen));
    }
    match &reading.banner {
        None => "Not checked yet".into(),
        Some(UpdateBanner::UpToDate | UpdateBanner::Ready { .. }) => "Up to date".into(),
        Some(UpdateBanner::Available { display, .. }) => {
            format!("{display} is available: it is not installed")
        }
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

fn last_check(chosen: Channel, reading: &Reading) -> Option<String> {
    if matches!(reading.phase, Phase::Downloading(_)) || chosen != reading.channel {
        return None;
    }
    let Checked { at, answered } = reading.checked?;
    Some(match answered {
        true => format!("checked at {}", clock(at)),
        false => format!("{} did not answer at {}", title(reading.channel), clock(at)),
    })
}

pub(super) fn clock(wall: i64) -> String {
    let local = (wall + crate::runtime::local_offset(wall)).rem_euclid(86_400);
    format!("{:02}:{:02}", local / 3_600, local / 60 % 60)
}
