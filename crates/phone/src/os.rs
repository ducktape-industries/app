//! What a phone gives the host core (`contract::OsServices`), and the one
//! place the phone shell names an OS: an arm per phone, the shape of gpui's
//! `current_platform`. Each arm is also where its launcher enters: the C ABI
//! `main` the iOS launcher calls ([`ios`]), the JNI entries of the Android
//! Activity ([`android`]).
//!
//! Directories are the app container's, handed in by the launcher (as the
//! Expo core takes its `data_dir`); keys are the Expo build's ([`crate::key`]);
//! banners are not posted yet (a later cut posts them through
//! `UNUserNotificationCenter` / `NotificationManager`); no node runs here, so
//! no invite is minted.

#[cfg(target_os = "android")]
pub mod android;
/// iOS: the launcher's C entry ([`ios::ffi`]); gpui_ios answers the rest.
#[cfg(target_os = "ios")]
pub mod ios {
    pub mod ffi;
}

use ducktape_host::contract::{
    Banners, Dirs, Inbound, KeyRefusal, KeyStorage, Limits, MintInvite, OsFacts, OsServices,
    Preload, RunnerConfig, Signal, SoftKeyboard, ViewRunner,
};
use ducktape_host::motion;
use ducktape_host::runtime::Answered;
use ducktape_host::runtime::notify::{Click, Notice};
use gpui_kit::{App, Context, Subscription, Window, WindowInsets};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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

/// Banners are not posted yet: `post` answers that nothing went up.
struct NoBanners;

impl Banners for NoBanners {
    fn post(&self, _: &Notice, _: Option<Click>) -> bool {
        false
    }

    fn withdraw(&self, _: &str) {}
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
        &NoBanners
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
        }
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
/// [`android`]). On iOS it also keeps the status bar's clock readable over
/// the system's appearance, light content on dark (Expo `app/_layout.tsx`
/// `<StatusBar style={dark ? 'light' : 'dark'} />`); Android's theme flips
/// it itself (`values-night/themes.xml`).
pub fn follow<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Vec<Subscription> {
    let _ = &window;
    #[cfg(target_os = "ios")]
    {
        fn status_bar(window: &Window) {
            use gpui_ios::StatusBarContentStyle as Style;
            gpui_ios::ios::set_status_bar_style(match window.appearance() {
                gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark => {
                    Style::Light
                }
                gpui_kit::WindowAppearance::Light | gpui_kit::WindowAppearance::VibrantLight => {
                    Style::Dark
                }
            });
        }
        status_bar(window);
        vec![
            cx.on_app_lifecycle(|phase, _| send(Signal::Lifecycle(phase))),
            cx.on_memory_warning(|_| send(Signal::MemoryWarning)),
            cx.observe_window_appearance(window, |_, window, _| status_bar(window)),
        ]
    }
    #[cfg(target_os = "android")]
    {
        // the window active is the app in the foreground (SPEC §7.5): the
        // session's status poll starts again on it
        vec![cx.observe_window_activation(window, |_, window, _| {
            send(Signal::Lifecycle(if window.is_window_active() {
                gpui_kit::AppLifecyclePhase::Foreground
            } else {
                gpui_kit::AppLifecyclePhase::Background
            }))
        })]
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = cx;
        Vec::new()
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
