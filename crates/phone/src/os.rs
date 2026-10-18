//! What a phone gives the host core (`contract::OsServices`), and the one
//! place the phone shell names an OS: an arm per phone, the shape of gpui's
//! `current_platform`. Each arm is also where its launcher enters: the C ABI
//! `main` the iOS launcher calls ([`ios`]), the JNI entries of the Android
//! Activity ([`android`]).
//!
//! Directories are the app container's, handed in by the launcher (as the
//! Expo core takes its `data_dir`); keys are the Expo build's ([`crate::key`]);
//! banners are the OS's own while the app is in front ([`PhoneBanners`]); no
//! node runs here, so no invite is minted.

#[cfg(target_os = "android")]
pub mod android;
/// iOS: the launcher's C entry ([`ios::ffi`]); gpui_ios answers the rest.
#[cfg(target_os = "ios")]
pub mod ios {
    pub mod ffi;

    /// Which of Apple's servers this build's APNs tokens belong to: the
    /// `aps-environment` of the provisioning profile the app was signed
    /// with (`embedded.mobileprovision` beside the executable, its plist in
    /// the clear inside the signed blob): `development` is the sandbox, as
    /// an Xcode build on a device is; `production` is TestFlight's and the
    /// App Store's, which ships no profile at all.
    pub fn apns_environment() -> &'static str {
        static ENV: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        ENV.get_or_init(|| {
            let profile = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("embedded.mobileprovision")));
            let bytes = profile
                .and_then(|profile| std::fs::read(profile).ok())
                .unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes);
            let value = text
                .find("<key>aps-environment</key>")
                .and_then(|at| {
                    let rest = &text[at..];
                    rest.find("<string>")
                        .map(|open| &rest[open + "<string>".len()..])
                })
                .and_then(|rest| rest.split('<').next())
                .map(str::trim);
            match value {
                Some("development") => "sandbox",
                _ => "production",
            }
        })
    }
}

use ducktape_host::backend::Appearance;
use ducktape_host::contract::{
    Banners, Dirs, Inbound, KeyRefusal, KeyStorage, Limits, MintInvite, OsFacts, OsServices,
    Preload, RunnerConfig, Signal, SoftKeyboard, ViewRunner,
};
use ducktape_host::motion;
use ducktape_host::runtime::Answered;
use ducktape_host::runtime::notify::{Click, Notice, NoticeKind};
use gpui_kit::{App, AppLifecyclePhase, Context, Subscription, Window, WindowInsets};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// What the phone feeds the host after start (`contract::Inbound`), kept
/// for the OS callbacks that arrive outside gpui (Android's Java thread).
static INBOUND: OnceLock<Inbound> = OnceLock::new();

/// Keeps the host's inbound sender for [`send`]. Once: the host starts once
/// per process.
pub fn install_inbound(inbound: Inbound) {
    INBOUND.get_or_init(|| inbound);
}

/// Hands `signal` to the host; dropped before the host has started (the
/// state it nudges is read again when the window first draws).
pub fn send(signal: Signal) {
    if let Some(inbound) = INBOUND.get() {
        inbound.send(signal);
    }
}

/// The chain in hand while the session is connected, empty while none is,
/// as the shell last published it ([`hold_chain`]): what the OS's own
/// threads read (a push that comes while the app is in front, a banner's
/// payload) without reaching the app.
static IN_HAND: Mutex<String> = Mutex::new(String::new());

/// The app in front of the person, as the OS's lifecycle last said
/// ([`follow`]); it starts there.
static IN_FRONT: AtomicBool = AtomicBool::new(true);

/// The chain in hand now (`chain`), `""` while the session is not
/// connected: the shell's word, on each move of the session.
pub(crate) fn hold_chain(chain: &str) {
    let mut held = IN_HAND.lock().unwrap_or_else(PoisonError::into_inner);
    if *held != chain {
        *held = chain.to_owned();
    }
}

fn in_hand() -> String {
    IN_HAND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The app's place on screen, to the host and kept for the banners
/// ([`IN_FRONT`]): in front until it is in the background or its scene
/// is gone (an inactive app, under Control Centre, is still on screen).
#[cfg_attr(
    not(any(target_os = "ios", target_os = "android")),
    expect(dead_code, reason = "only a phone's lifecycle calls it")
)]
fn lifecycle(phase: AppLifecyclePhase) {
    let front = !matches!(
        phase,
        AppLifecyclePhase::Background | AppLifecyclePhase::Disconnected
    );
    IN_FRONT.store(front, Ordering::Relaxed);
    send(Signal::Lifecycle(phase));
}

/// Links the OS handed the app (an Android intent's data, an iOS scene's
/// URL), from any thread, for the shell to open; one that comes before the
/// shell is up waits here.
static LINKS: OnceLock<(
    async_channel::Sender<String>,
    async_channel::Receiver<String>,
)> = OnceLock::new();

fn links() -> &'static (
    async_channel::Sender<String>,
    async_channel::Receiver<String>,
) {
    LINKS.get_or_init(async_channel::unbounded)
}

/// Hands `url` to the shell ([`links_to_open`]).
pub fn link(url: String) {
    if let Err(error) = links().0.try_send(url) {
        tracing::error!(target: "ducktape::phone", %error, "a link the OS handed over was lost");
    }
}

/// A program's notice the OS showed (`App.swift`'s `Ring`, `Ring.kt`), as
/// the push server carried it: its payload is `Push::payload`'s in
/// `crates/pushd` (`kind`, `chain`, `program`, `tag`, `title`, `body`,
/// `link`, `until`; on Android every value a string, FCM's data), the
/// banner its centre let through. This phone's own banner carries the same
/// ([`banner_payload`]), and `at`, its row's time.
#[derive(Clone, Debug, PartialEq)]
pub struct OsNotice {
    /// The chain whose log it goes into, as links spell it.
    pub chain: String,
    /// The banner the push server's centre let through.
    pub notice: Notice,
    /// Unix seconds the OS showed it at.
    pub at: i64,
    /// Handed over as it came while the app was in front, and the OS
    /// showed nothing: this phone's own notifier banners it
    /// ([`OsNotice::shows_in_front`]).
    pub front: bool,
    /// The person opened it from the OS.
    pub opened: bool,
}

impl OsNotice {
    /// `payload` (JSON) as the OS kept it, shown at `at` unless it names
    /// its own time (this phone's banner: its row's, which the OS's comes a
    /// moment after); `None` for one that is no program's notice (a call, a
    /// withdraw, "N more", not the carrier's).
    pub fn of(payload: &str, at: i64, front: bool, opened: bool) -> Option<Self> {
        let payload: serde_json::Value = serde_json::from_str(payload).ok()?;
        let text = |key: &str| payload[key].as_str().unwrap_or_default().to_owned();
        if text("kind") != "notice" || text("chain").is_empty() || text("program").is_empty() {
            return None;
        }
        let own = payload["at"].as_i64().or_else(|| text("at").parse().ok());
        let at = own.unwrap_or(at);
        Some(Self {
            chain: text("chain"),
            notice: Notice {
                title: text("title"),
                body: text("body"),
                tag: text("tag"),
                program: text("program"),
                link: text("link"),
                ..Notice::default()
            },
            at,
            front,
            opened,
        })
    }

    /// Whether the OS shows one that came while the app was in front by
    /// the OS's word: unless this phone banners it itself, which it does
    /// with the app `in_front` by its own lifecycle on the chain of the
    /// notice in hand (`in_hand`, `""`: none connected). Those are the
    /// notifiers this phone runs, and what they post is the banner
    /// ([`PhoneBanners`]: the same two words); they also leave alone what
    /// the person is looking at (`host.screen`), which the push server
    /// cannot see.
    fn shows_in_front(&self, in_hand: &str, in_front: bool) -> bool {
        self.front && !(in_front && self.chain == in_hand)
    }
}

/// The OS's notices handed over ([`os_notice`]), from any thread, for the
/// shell to log; those that come before the shell is up wait here.
static OS_NOTICES: OnceLock<(
    async_channel::Sender<OsNotice>,
    async_channel::Receiver<OsNotice>,
)> = OnceLock::new();

fn os_notice_channel() -> &'static (
    async_channel::Sender<OsNotice>,
    async_channel::Receiver<OsNotice>,
) {
    OS_NOTICES.get_or_init(async_channel::unbounded)
}

/// Hands the OS's notice `payload` (JSON), shown at `at`, to the shell
/// ([`os_notices`]); one that is no program's notice is let go. Whether the
/// OS shows one that came while the app is in front
/// ([`OsNotice::shows_in_front`]). From the OS's own thread: it reads only
/// what the shell published.
pub fn os_notice(payload: &str, at: i64, front: bool, opened: bool) -> bool {
    let Some(mut notice) = OsNotice::of(payload, at, front, opened) else {
        tracing::debug!(target: "ducktape::phone", "an OS notice that is no program's");
        return false;
    };
    let shows = notice.shows_in_front(&in_hand(), IN_FRONT.load(Ordering::Relaxed));
    // shown, it is logged as any the OS showed
    notice.front &= !shows;
    if let Err(error) = os_notice_channel().0.try_send(notice) {
        tracing::error!(target: "ducktape::phone", %error, "an OS notice was lost");
    }
    shows
}

/// Takes the OS's notices under `tag` (the push server's banner tag,
/// `<program>/<tag>`; each notice is its own notification there) out of its
/// list: their row was read or cleared here, or its program said it was
/// seen. Nothing when none stands there. Any thread.
pub fn withdraw_notice(tag: &str) {
    #[cfg(target_os = "ios")]
    ios::ffi::withdraw_notice(tag);
    #[cfg(target_os = "android")]
    android::jni::withdraw_notice(tag);
    #[cfg(test)]
    WITHDRAWN.with(|withdrawn| withdrawn.borrow_mut().push(tag.to_owned()));
    // a Linux build keeps no OS list
    #[cfg(not(any(target_os = "ios", target_os = "android", test)))]
    let _ = tag;
}

#[cfg(test)]
thread_local! {
    /// What [`withdraw_notice`] took out of the OS's list, in a test.
    pub static WITHDRAWN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The OS's notices to log, in the order they came.
pub(crate) fn os_notices() -> async_channel::Receiver<OsNotice> {
    os_notice_channel().1.clone()
}

/// This device's push tokens by kind (`voip`: PushKit's, as hex; `apns`:
/// the remote-notification token alerts and a withdraw are pushed to, as
/// hex; `fcm`: Firebase's), as the OS last handed them over, for the
/// registrar ([`crate::push`]) that hands them to the push server.
static PUSH_TOKENS: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// One item each time the tokens moved, for the registrar; a bounded
/// channel of one, so moves coalesce.
static PUSH_MOVED: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
    OnceLock::new();

fn push_moved() -> &'static (async_channel::Sender<()>, async_channel::Receiver<()>) {
    PUSH_MOVED.get_or_init(|| async_channel::bounded(1))
}

/// Keeps `token` as this device's push token of `kind`; an empty one is
/// withdrawn. The registrar is woken when the kept one moved.
pub fn push_token(kind: &str, token: String) {
    // the token addresses this device: kept out of the log
    tracing::debug!(target: "ducktape::phone", kind, "this device's push token");
    let moved = {
        let mut tokens = PUSH_TOKENS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match token.is_empty() {
            true => tokens.remove(kind).is_some(),
            false => tokens.insert(kind.to_owned(), token.clone()) != Some(token),
        }
    };
    if moved {
        // a full channel already holds a wake
        push_moved().0.try_send(()).ok();
    }
}

/// Wakes once per move of the tokens ([`push_token`]).
pub fn push_tokens_moved() -> async_channel::Receiver<()> {
    push_moved().1.clone()
}

/// Which of Apple's servers this build's APNs tokens belong to
/// (`production` or `sandbox`); an Android build's tokens have no such
/// side, and the registrar sends none for them.
pub fn push_environment() -> &'static str {
    #[cfg(target_os = "ios")]
    {
        ios::apns_environment()
    }
    #[cfg(not(target_os = "ios"))]
    {
        "production"
    }
}

/// This device's push tokens by kind ([`push_token`]).
pub fn push_tokens() -> std::collections::BTreeMap<String, String> {
    PUSH_TOKENS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// The links to open, in the order they came.
pub(crate) fn links_to_open() -> async_channel::Receiver<String> {
    links().1.clone()
}

/// The host's directories inside the app container: `documents` is the
/// directory the OS keeps and backs up (iOS `Documents`, Android
/// `filesDir`), `caches` the one it may purge (iOS `Library/Caches`, Android
/// `cacheDir`).
///
/// `remotes` is pinned to `<documents>/remotes`, where the Expo build kept
/// its keyrings (mobile `node.rs:156`, `index.ts:15`): the keyring's name is
/// the key's alias, so any other place loses the key on the update (SPEC
/// §4.8). The rest is the host's own, under `ducktape/`, beside the Expo
/// build's `store` and never in it. `runtime` (the AX door's file) is the
/// cache directory, where the walk reads it (SPEC §11.2).
pub fn dirs(documents: &Path, caches: &Path) -> Dirs {
    let own = documents.join("ducktape");
    let cache = caches.join("ducktape");
    Dirs {
        config: own.join("config"),
        data: own.join("data"),
        remotes: documents.join("remotes"),
        runtime: cache.clone(),
        cache,
        views_override: None,
    }
}

/// Makes every directory in `dirs`, the runtime one private to this app
/// (the `Dirs` rule: the host never makes it, and writes the door's token
/// there plainly).
fn made(dirs: &Dirs) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    for dir in [&dirs.config, &dirs.data, &dirs.remotes, &dirs.cache] {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dirs.runtime)?;
    std::fs::set_permissions(&dirs.runtime, std::fs::Permissions::from_mode(0o700))
}

/// The phone's `OsServices`.
pub struct PhoneOs {
    dirs: Dirs,
    reader: Arc<dyn noded_client::Signer>,
}

impl PhoneOs {
    /// The phone's [`OsFacts`].
    fn facts_of() -> OsFacts {
        OsFacts {
            mac_modifiers: false,
            // `getrusage`'s `ru_maxrss`: bytes on Darwin, kibibytes on Linux
            rss_unit: if cfg!(target_os = "ios") { 1 } else { 1024 },
            touch_first: true,
            // the Expo build's label for this key (mobile `node.rs:344`)
            device_label: "Phone",
            device_noun: "phone",
            one_layout: true,
            floats: false,
            motion: if cfg!(target_os = "android") {
                motion::ANDROID
            } else {
                motion::IOS
            },
            // iOS's alert prompt, Android 13's POST_NOTIFICATIONS
            os_asks_notices: true,
        }
    }

    /// The services over the container's `documents` and `caches`, every
    /// directory made. A sentence when a directory cannot be made or the
    /// OS gives no randomness for the reader key.
    pub(crate) fn open(documents: PathBuf, caches: PathBuf) -> Result<Self, String> {
        let dirs = dirs(&documents, &caches);
        made(&dirs).map_err(|error| format!("Could not make the app's folders: {error}"))?;
        let reader =
            crate::key::Reader::new().ok_or("This phone gave no randomness for a reader key.")?;
        Ok(Self {
            dirs,
            reader: Arc::new(reader),
        })
    }
}

/// The OS's own banners (iOS's, Android's notification), for what this
/// phone's notifiers post while the app is in front (owner 10-17): what is
/// a notice, for whom, its words, tag, link and withdrawal are each
/// program's, decided in its notifier; the host's centre lets it through
/// (the person's word, the burst); this only shows it. While the app is
/// not in front, the push server shows the same notices: none here then,
/// so none shows twice. A call is the ring's (CallKit, `Ring.kt`), never a
/// banner. A withdraw (its program saw it, or its `until`) takes what
/// stands under the tag out of the OS's list, this phone's and the push
/// server's ([`withdraw_notice`]).
struct PhoneBanners;

impl Banners for PhoneBanners {
    fn post(&self, notice: &Notice, click: Option<Click>) -> bool {
        let row = match click {
            Some(Click::Row(id)) => Some(row_at(id)),
            _ => None,
        };
        let front = IN_FRONT.load(Ordering::Relaxed);
        match banner_payload(notice, row, &in_hand(), front) {
            Some(payload) => show_banner(&payload.to_string()),
            None => false,
        }
    }

    fn withdraw(&self, tag: &str) {
        withdraw_notice(tag);
    }
}

/// When the app's centre logged row `id` (now, if it is gone).
fn row_at(id: u64) -> i64 {
    let center = ducktape_host::runtime::notify::center().lock();
    let row = center.entries().find(|entry| entry.id == id);
    row.map_or_else(ducktape_host::runtime::notify::wall, |entry| entry.at)
}

/// What this phone's banner of `notice` carries (its `userInfo`, its
/// notification's extras): the push server's payload for it
/// (`Push::show` in crates/pushd) on `chain`, so a tap on it is read and
/// opened as a pushed one's is ([`OsNotice`]), under the same id (a late
/// push of it replaces it, never stands beside it), and `at`, the time of `row`,
/// the row it opens. "N more" opens no row: no program's notice, a tap on
/// it only brings the app up. None for a call, with no chain in hand, or
/// with the app not in `front`.
fn banner_payload(
    notice: &Notice,
    row: Option<i64>,
    chain: &str,
    front: bool,
) -> Option<serde_json::Value> {
    if !front || chain.is_empty() || notice.kind == NoticeKind::Call {
        return None;
    }
    let mut payload = serde_json::json!({
        "kind": "more",
        "chain": chain,
        "program": notice.program,
        "tag": notice.tag,
        "title": notice.title,
        "body": notice.body,
        "id": notice.os_id(),
    });
    if let Some(at) = row {
        payload["kind"] = "notice".into();
        payload["link"] = notice.link.clone().into();
        payload["until"] = notice.until.into();
        payload["at"] = at.into();
    }
    Some(payload)
}

/// `payload` ([`banner_payload`]) up as the OS's own banner; whether it
/// was handed to the OS.
fn show_banner(payload: &str) -> bool {
    #[cfg(target_os = "ios")]
    {
        ios::ffi::banner(payload)
    }
    #[cfg(target_os = "android")]
    {
        android::jni::banner(payload)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = payload;
        false
    }
}

impl OsServices for PhoneOs {
    fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    /// Always an OS-kept key (made on first ask): a phone never had the
    /// desktop's password-locked key files.
    fn device_key(
        &self,
        keyring: &str,
    ) -> Result<Option<Arc<dyn noded_client::Signer>>, KeyRefusal> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            let key = crate::key::DeviceKey::open(keyring).map_err(KeyRefusal::Denied)?;
            // the public half only: which key a keyring opened, in the log
            // the update from the Expo build is checked against (SPEC §4.8)
            let public = noded_client::Signer::public_key(&key);
            let public: String = public.iter().map(|byte| format!("{byte:02x}")).collect();
            tracing::info!(target: "ducktape::phone", keyring, %public, "this device's key opened");
            Ok(Some(Arc::new(key)))
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = keyring;
            Err(KeyRefusal::NoStore(NO_STORE.into()))
        }
    }

    fn key_storage(&self, keyring: &str) -> Result<KeyStorage, KeyRefusal> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            crate::key::storage(keyring).map_err(KeyRefusal::Broken)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = keyring;
            Err(KeyRefusal::NoStore(NO_STORE.into()))
        }
    }

    fn reader(&self) -> Arc<dyn noded_client::Signer> {
        self.reader.clone()
    }

    fn banners(&self) -> &dyn Banners {
        &PhoneBanners
    }

    /// gpui_ios answers the safe area and the keyboard itself; gm's host
    /// path answers neither, so Android's come from the Activity
    /// ([`android::insets`]).
    fn insets(&self, window: &Window) -> WindowInsets {
        #[cfg(target_os = "android")]
        {
            android::insets(window)
        }
        #[cfg(not(target_os = "android"))]
        {
            window.insets()
        }
    }

    /// gpui_ios raises and lowers the keyboard with the focused field (the
    /// UIKit text contract); gm's host path is told.
    fn soft_keyboard(&self, window: &Window, want: SoftKeyboard) {
        let _ = window;
        #[cfg(target_os = "android")]
        android::soft_keyboard(want);
        #[cfg(not(target_os = "android"))]
        let _ = want;
    }

    fn facts(&self) -> OsFacts {
        Self::facts_of()
    }

    fn limits(&self) -> Limits {
        Limits {
            // one Cranelift compile peaks at 200-315 MB; the OS's memory
            // killer is the owner of this bound (SPEC §4.5)
            loaders: 1,
            preload: Preload::Seat,
            // iOS's views run in WebKit's content process, which can die
            // under them (SPEC §6.5); Android's in this one, with the app
            snapshot_every: cfg!(target_os = "ios").then_some(std::time::Duration::from_secs(1)),
            snapshot_to_disk: true,
        }
    }

    /// No view ships in the app yet (the phone's `views.lock`, SPEC §4.7):
    /// every one is the network's.
    fn bundled_view(&self, _: &abi::BlobId) -> Option<Vec<u8>> {
        None
    }

    fn mint_invite(&self, _: MintInvite) -> Option<Answered> {
        None
    }
}

/// What runs the views (`contract::ViewRunner`): JavaScriptCore's page on
/// iOS, where no process of ours may JIT (`ducktape_host_jsc`); wasmtime
/// elsewhere, its traps without signals on Android (which make every arm64
/// guest trap a clean error on libc 0.2.189: SPEC C3, S0-android §A).
/// Either keeps its code under the cache ([`code_cache`]). A runner iOS
/// cannot make (not on the main thread, which the launch is) stops the
/// phone with the reason logged.
pub fn runner(dirs: &Dirs) -> Arc<dyn ViewRunner> {
    let config = RunnerConfig {
        signals_based_traps: !cfg!(target_os = "android"),
        cache_dir: code_cache(&dirs.cache),
        compile_apart: None,
    };
    #[cfg(target_os = "ios")]
    {
        match ducktape_host_jsc::JscRunner::new(config) {
            Ok(runner) => Arc::new(runner),
            Err(failure) => {
                tracing::error!(target: "ducktape::phone", %failure, "no view runner");
                std::process::abort();
            }
        }
    }
    #[cfg(not(target_os = "ios"))]
    {
        Arc::new(ducktape_host::runner::WasmtimeRunner::new(config))
    }
}

/// Where the runner keeps its code under `cache`. On iOS, one folder per
/// app build (`DUCKTAPE_APP_BUILD`, the commit and any uncommitted change),
/// the other builds' removed: the code kept there is the meter pass's
/// output, and another build's pass may differ; a build that knows no
/// commit (no git) keeps none. wasmtime keys its own cache by its version
/// and settings, so it shares one across builds.
fn code_cache(cache: &Path) -> Option<PathBuf> {
    let dir = cache.join("view-code");
    match cfg!(target_os = "ios") {
        true => option_env!("DUCKTAPE_APP_BUILD").map(|build| per_build(&dir, build)),
        false => Some(dir),
    }
}

/// `dir`'s folder for `build`, every other one under `dir` removed.
fn per_build(dir: &Path, build: &str) -> PathBuf {
    for old in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if old.file_name() != build {
            // a folder that stays is read by no build: the OS purges it
            let _gone = std::fs::remove_dir_all(old.path());
        }
    }
    dir.join(build)
}

/// What a build for neither phone answers for a key: such a build exists
/// only for the workspace's own checks and tests.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
const NO_STORE: &str = "This build runs on no phone, so it has no key store.";

/// Starts the host over the container's `documents` and `caches`, once
/// the app's thread runs: what both launchers call. A phone that cannot
/// make its own folders cannot run; it stops with the reason logged (SPEC
/// §13.3: a host panic on a phone aborts).
#[cfg_attr(
    not(any(target_os = "ios", target_os = "android")),
    expect(dead_code, reason = "only a phone's launcher enters")
)]
pub(crate) fn start(documents: PathBuf, caches: PathBuf, cx: &mut App) {
    match PhoneOs::open(documents, caches) {
        Ok(os) => crate::shell::launch(Arc::new(os), cx),
        Err(error) => {
            tracing::error!(target: "ducktape::phone", %error, "the phone could not start");
            std::process::abort();
        }
    }
}

/// Follows the app's place on screen into the host: iOS's own phases and
/// memory warnings; Android's window activation, which gm drives from the
/// Activity's resume and pause (its memory warning comes from the Activity,
/// [`android`]).
pub fn follow<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Vec<Subscription> {
    let _ = &window;
    #[cfg(target_os = "ios")]
    {
        vec![
            cx.on_app_lifecycle(|phase, _| lifecycle(phase)),
            cx.on_memory_warning(|_| send(Signal::MemoryWarning)),
        ]
    }
    #[cfg(target_os = "android")]
    {
        // the window active is the app in the foreground (SPEC §7.5): the
        // session's status poll starts again on it
        vec![cx.observe_window_activation(window, |_, window, _| {
            lifecycle(if window.is_window_active() {
                AppLifecyclePhase::Foreground
            } else {
                AppLifecyclePhase::Background
            })
        })]
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = cx;
        Vec::new()
    }
}

/// Keeps the system bars readable over the app's appearance (`dark`: the
/// one the Account tab chose, or the system's): light content on dark (Expo
/// `app/_layout.tsx` `<StatusBar style={dark ? 'light' : 'dark'} />`). iOS:
/// the status bar's style; Android: the status and navigation bars' icons
/// (`MainActivity.systemBars`; the theme's `values-night` flip follows the
/// system only).
pub fn system_bars(dark: bool) {
    #[cfg(target_os = "ios")]
    gpui_ios::ios::set_status_bar_style(match dark {
        true => gpui_ios::StatusBarContentStyle::Light,
        false => gpui_ios::StatusBarContentStyle::Dark,
    });
    #[cfg(target_os = "android")]
    android::jni::system_bars(dark);
    let _ = dark;
}

/// Puts the appearance the Account tab chose on the app's iOS windows
/// (`overrideUserInterfaceStyle`), so what UIKit draws for the app takes it
/// too: the keyboard, its own alerts and sheets, and the trait collection
/// gpui_ios reads `window.appearance()` from. System leaves them to the
/// system (`Unspecified`). Called before the theme reads the window, which
/// under System it must read without an override; a trait change UIKit
/// lands later is heard as the window's appearance changing. Android draws
/// its keyboard by the keyboard's own theme: nothing there.
pub fn interface_style(appearance: Appearance) {
    #[cfg(target_os = "ios")]
    {
        use objc2_ui_kit::{UIApplication, UIUserInterfaceStyle, UIWindowScene};
        let Some(main) = objc2::MainThreadMarker::new() else {
            return;
        };
        let style = match appearance {
            Appearance::System => UIUserInterfaceStyle::Unspecified,
            Appearance::Light => UIUserInterfaceStyle::Light,
            Appearance::Dark => UIUserInterfaceStyle::Dark,
        };
        for scene in UIApplication::sharedApplication(main).connectedScenes() {
            if let Ok(scene) = scene.downcast::<UIWindowScene>() {
                for window in scene.windows() {
                    window.setOverrideUserInterfaceStyle(style);
                }
            }
        }
    }
    let _ = appearance;
}

/// The app's memory footprint now and at its peak, in bytes, on iOS:
/// Mach's `TASK_VM_INFO` `phys_footprint` and `ledger_phys_footprint_peak`,
/// the figure iOS's memory limit counts and Xcode's memory gauge shows
/// (dirty and compressed pages, not shared clean ones). Each is `None`
/// where the kernel's answer does not reach it, and both are elsewhere
/// (Android's resident figure is the host's, `/proc/self/statm`).
#[cfg_attr(
    target_os = "ios",
    allow(unsafe_code, reason = "Mach's task_info on this task")
)]
pub fn footprint_bytes() -> (Option<u64>, Option<u64>) {
    #[cfg(target_os = "ios")]
    {
        use mach2::task_info::{TASK_VM_INFO, task_vm_info};
        let mut info = task_vm_info::default();
        let mut count =
            (size_of::<task_vm_info>() / size_of::<mach2::vm_types::natural_t>()) as u32;
        // SAFETY: this task's own port, and an out struct of `count`
        // naturals, which the kernel fills no further than
        let answered = unsafe {
            mach2::task::task_info(
                mach2::traps::mach_task_self(),
                TASK_VM_INFO,
                (&raw mut info).cast(),
                &mut count,
            )
        };
        if answered != mach2::kern_return::KERN_SUCCESS {
            return (None, None);
        }
        // an older kernel answers a shorter struct: a field past its answer is not had
        let filled = |end: usize| end <= count as usize * size_of::<mach2::vm_types::natural_t>();
        let now = std::mem::offset_of!(task_vm_info, phys_footprint) + size_of::<u64>();
        let peak =
            std::mem::offset_of!(task_vm_info, ledger_phys_footprint_peak) + size_of::<i64>();
        (
            filled(now).then_some(info.phys_footprint),
            filled(peak)
                .then(|| u64::try_from(info.ledger_phys_footprint_peak).ok())
                .flatten(),
        )
    }
    #[cfg(not(target_os = "ios"))]
    (None, None)
}

/// The system, as a sentence names it ("Light and dark come from iOS's
/// setting."): Expo `platform.system`.
pub const SYSTEM: &str = if cfg!(target_os = "android") {
    "Android"
} else {
    "iOS"
};

/// The key store, named where it does not say how it keeps the key: Expo
/// `platform.keyStore`.
pub const KEY_STORE: &str = if cfg!(target_os = "android") {
    "Android Keystore"
} else {
    "iOS Keychain"
};

/// This phone, as the Account tab names it (Expo `platform.phone`): the model
/// and the system's version on Android; on iOS, where the Expo build read
/// no model name, "iPhone" and the system's version (UIKit's
/// `systemVersion`: major.minor, and the patch when there is one).
pub fn phone() -> String {
    #[cfg(target_os = "android")]
    {
        android::phone()
    }
    #[cfg(not(target_os = "android"))]
    {
        #[cfg(target_os = "ios")]
        let version = {
            let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
            match version.patchVersion {
                0 => format!("{}.{}", version.majorVersion, version.minorVersion),
                patch => format!("{}.{}.{patch}", version.majorVersion, version.minorVersion),
            }
        };
        // a build for no phone (the workspace's checks) has no version
        #[cfg(not(target_os = "ios"))]
        let version = "";
        format!("iPhone · iOS {version}")
    }
}

/// Brings up a refused camera's or microphone's settings page
/// (`render::av::settings_page`): a link the system opens on iOS; on
/// Android the Activity starts it, gm's platform opening no link (its
/// `open_url` only logs).
pub fn open_settings(page: &str, cx: &App) {
    #[cfg(target_os = "android")]
    {
        let _ = cx;
        android::jni::open_settings(page);
    }
    #[cfg(not(target_os = "android"))]
    cx.open_url(page);
}

/// What BACK does when the stack has nothing left to pop: Android puts the
/// app behind the others; iOS has no such gesture.
pub fn back_at_root() {
    #[cfg(target_os = "android")]
    android::jni::move_task_to_back();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A push token is kept by its kind, the next one in its place, and an
    /// empty one withdraws it.
    #[test]
    fn push_tokens_are_kept_by_kind() {
        push_token("test-voip", "aa".into());
        push_token("test-voip", "bb".into());
        push_token("test-fcm", "f".into());
        assert_eq!(
            push_tokens().get("test-voip").map(String::as_str),
            Some("bb")
        );
        push_token("test-voip", String::new());
        assert!(!push_tokens().contains_key("test-voip"));
        assert!(push_tokens().contains_key("test-fcm"));
        // the registrar was woken, once for the moves so far
        assert!(push_tokens_moved().try_recv().is_ok());
        assert!(push_tokens_moved().try_recv().is_err());
        push_token("test-fcm", "f".into());
        assert!(
            push_tokens_moved().try_recv().is_err(),
            "an unmoved token woke it"
        );
    }

    /// The carrier's notice reads the same as Android keeps it (FCM's
    /// strings) as iOS does (JSON values); its call and its withdraw are no
    /// notice for the list, nor is a payload that is not the carrier's.
    #[test]
    fn only_the_carriers_notice_payload_is_a_notice() {
        let fcm = r#"{"kind":"notice","chain":"c#1","program":"chat","tag":"chat/#general",
            "title":"ada","body":"hey","link":"duck://c/chat/general/7","until":"0","quiet":"1"}"#;
        let shown = OsNotice::of(fcm, 5, false, true).expect("a notice");
        assert_eq!(
            (
                shown.chain.as_str(),
                shown.notice.program.as_str(),
                shown.notice.tag.as_str()
            ),
            ("c#1", "chat", "chat/#general")
        );
        assert_eq!(shown.notice.link, "duck://c/chat/general/7");
        assert!((shown.at, shown.front, shown.opened) == (5, false, true));
        let call = r#"{"kind":"call","chain":"c#1","program":"huddle","tag":"huddle/call/1"}"#;
        assert_eq!(OsNotice::of(call, 5, false, false), None);
        assert_eq!(
            OsNotice::of(
                r#"{"kind":"withdraw","tag":"chat/#general"}"#,
                5,
                false,
                false
            ),
            None
        );
        assert_eq!(
            OsNotice::of(r#"{"aps":{"alert":"hi"}}"#, 5, false, false),
            None
        );
    }

    /// The push server's payload for a direct message from `title` on
    /// `chain` (`Push::payload`, as the OS keeps it), under chat's tag for
    /// that room.
    fn pushed(chain: &str, title: &str) -> String {
        serde_json::json!({
            "kind": "notice", "chain": chain, "program": "chat", "tag": format!("chat/@{title}"),
            "title": title, "body": "said", "link": "", "until": 0,
        })
        .to_string()
    }

    /// A push the OS hands over as in front shows unless this phone
    /// banners it itself: the app in front by its own lifecycle, on the
    /// push's chain. Another chain's, one with none connected, and one the
    /// app's lifecycle already put behind (iOS asks a moment after a move
    /// to the background) show. Delivered or opened, it is no front's.
    #[test]
    fn a_push_in_front_shows_unless_this_phone_banners_it() {
        let shows = |chain: &str, in_hand: &str, front: bool, in_front: bool| {
            let notice = OsNotice::of(&pushed(chain, "ada"), 5, front, false).expect("a notice");
            notice.shows_in_front(in_hand, in_front)
        };
        assert!(
            !shows("garage#1", "garage#1", true, true),
            "shown over its own notifier's banner"
        );
        assert!(
            shows("other#2", "garage#1", true, true),
            "another chain's was hidden"
        );
        assert!(
            shows("garage#1", "", true, true),
            "hidden with no chain connected"
        );
        assert!(
            shows("garage#1", "garage#1", true, false),
            "hidden with the app behind, where this phone banners nothing"
        );
        assert!(
            !shows("other#2", "garage#1", false, true),
            "a delivered one shown again"
        );
    }

    /// This phone's banner carries the push server's payload on the chain
    /// in hand and its row's time, so the OS's copy of it and a tap on it
    /// read as a pushed one's at that time (not the OS's, a moment later);
    /// "N more" opens no row and is no program's notice. A call, the app
    /// not in front, or no chain in hand: no banner.
    #[test]
    fn this_phones_banner_is_the_pushs_payload_at_its_rows_time() {
        let notice = Notice {
            title: "ada".into(),
            body: "said".into(),
            tag: "chat/#room".into(),
            program: "chat".into(),
            link: "duck://garage/chat/room/7".into(),
            ..Notice::default()
        };
        let payload = banner_payload(&notice, Some(100), "garage#1", true).expect("a banner");
        let opened = OsNotice::of(&payload.to_string(), 101, false, true).expect("a notice");
        assert_eq!((opened.chain.as_str(), opened.at), ("garage#1", 100));
        assert_eq!(opened.notice, notice);
        // the push's own id for it (Android shows none without one)
        assert_eq!(payload["id"], notice.os_id(), "not the push's id");
        // Android keeps every value a string (`Ring.banner`)
        let strings = payload.as_object().unwrap().iter();
        let strings: serde_json::Map<_, _> = strings
            .map(|(key, value)| (key.clone(), value.to_string().trim_matches('"').into()))
            .collect();
        let shade = OsNotice::of(
            &serde_json::Value::Object(strings).to_string(),
            101,
            false,
            false,
        );
        assert_eq!(
            shade.map(|it| it.at),
            Some(100),
            "the shade's copy at the OS's time"
        );
        assert_eq!(
            OsNotice::of(&pushed("garage#1", "ada"), 101, false, false).map(|it| it.at),
            Some(101)
        );

        let more = banner_payload(&notice, None, "garage#1", true).expect("a banner");
        assert_eq!(
            OsNotice::of(&more.to_string(), 101, false, true),
            None,
            "N more opened a row"
        );
        let call = Notice {
            kind: NoticeKind::Call,
            ..notice.clone()
        };
        assert_eq!(
            banner_payload(&call, Some(100), "garage#1", true),
            None,
            "a call bannered"
        );
        assert_eq!(
            banner_payload(&notice, Some(100), "garage#1", false),
            None,
            "bannered behind"
        );
        assert_eq!(
            banner_payload(&notice, Some(100), "", true),
            None,
            "bannered with no chain"
        );
    }

    /// What the person sees while the app is in front on a chain, with the
    /// phone's word on a program they said nothing of (`os_asks_notices`):
    /// each notice is one banner of the OS's. A push for the chain in hand
    /// shows nothing, and this phone's notifier's post of it is the banner;
    /// one for another chain shows itself; one the OS showed before the app
    /// came up is not bannered again when the notifier here catches up; a
    /// tap on this phone's banner reads its row.
    #[test]
    fn in_front_each_notice_is_one_os_banner() {
        use ducktape_host::runtime::notify::{CenterHandle, Permission, Settings};
        assert!(PhoneOs::facts_of().os_asks_notices);
        let settings = Settings {
            banners: true,
            in_front: false,
            burst: 6,
            views: Default::default(),
            unasked: Some(Permission::Allow),
        };
        let handle = CenterHandle::default();
        let mut center = handle.lock();
        let now = std::time::Instant::now();
        let post = |title: &str| view_wire::methods::Notification {
            title: title.into(),
            body: "said".into(),
            tag: format!("@{title}"),
            ..Default::default()
        };
        let mut banners = Vec::new();

        // in front on garage#1: ada's push and this phone's run of it
        let push = OsNotice::of(&pushed("garage#1", "ada"), 100, true, false).unwrap();
        if push.shows_in_front("garage#1", true) {
            banners.push("ada, pushed");
        }
        let (_, raised) = center.post(&settings, "chat", "Chat", post("ada"), now, 100);
        let row = center.entries().next().map(|row| (row.id, row.at));
        let ours = raised
            .and_then(|notice| banner_payload(&notice, row.map(|row| row.1), "garage#1", true));
        if ours.is_some() {
            banners.push("ada, this phone's");
        }
        // another chain's push
        if OsNotice::of(&pushed("other#2", "bob"), 110, true, false)
            .unwrap()
            .shows_in_front("garage#1", true)
        {
            banners.push("bob, pushed");
        }
        // cy's push the OS showed while the app was away, then the catch-up
        let shown = OsNotice::of(&pushed("garage#1", "cy"), 150, false, false).unwrap();
        center.carried(&settings, &shown.notice, shown.at, false);
        let (_, raised) = center.post(&settings, "chat", "Chat", post("cy"), now, 160);
        if raised.is_some() {
            banners.push("cy, this phone's");
        }
        assert_eq!(banners, ["ada, this phone's", "bob, pushed"]);

        // ada's banner tapped, the OS's time a moment after its row's
        let tapped = OsNotice::of(&ours.unwrap().to_string(), 101, false, true).unwrap();
        center.carried(&settings, &tapped.notice, tapped.at, tapped.opened);
        let ada = center
            .entries()
            .find(|entry| Some(entry.id) == row.map(|row| row.0));
        assert!(ada.is_some_and(|ada| ada.read), "tapped, and still unread");
        assert_eq!(
            center
                .entries()
                .filter(|entry| entry.title == "ada")
                .count(),
            1
        );
    }

    /// An app build keeps its code in a folder of its own, and the other
    /// builds' folders go: no build reads another build's metered code.
    #[test]
    fn each_app_build_keeps_its_code_apart() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join("view-code");
        let old = per_build(&dir, "a1");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("code.metered.wasm"), b"old pass").unwrap();
        assert_eq!(per_build(&dir, "a1"), old, "the same build reads its own");
        let new = per_build(&dir, "b2");
        assert_eq!(new, dir.join("b2"));
        assert!(!old.exists(), "another build's code stays");
    }

    /// The host's files sit under `ducktape/`, beside the Expo build's
    /// `store`; the keyrings stay where the Expo build put them; the runtime
    /// directory is made private.
    #[test]
    fn the_container_holds_the_expo_keyrings_and_the_hosts_own_files() {
        let container = tempfile::tempdir().unwrap();
        let (documents, caches) = (
            container.path().join("Documents"),
            container.path().join("Caches"),
        );
        let os = PhoneOs::open(documents.clone(), caches.clone()).unwrap();
        let dirs = os.dirs();
        assert_eq!(dirs.remotes, documents.join("remotes"));
        assert!(dirs.config.starts_with(documents.join("ducktape")));
        assert!(dirs.data.starts_with(documents.join("ducktape")));
        assert_eq!(dirs.runtime, caches.join("ducktape"));
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&dirs.runtime)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
