//! The app updates itself. One executable plays both parts of
//! [`app_update::step`]'s machine and shares nothing between them but
//! `state.json`:
//! - [`boot`], at the very top of `main` before gpui starts: reads the state,
//!   qualifies a staged release, flips to it (or back from one that never
//!   came up) and `exec`s the release that should run, same PID, argv passed
//!   through ([`boot`] module);
//! - the running app ([`channel`]): its first frame is the healthy signal,
//!   and from then on it checks the chosen channel (stable or nightly) at
//!   [`app_update::layout::BASE_URL`] now and hourly, downloads, verifies
//!   and stages the next release for the next start, says so in one desktop
//!   notice, and shows where it is in Settings › Updates;
//! - `ducktape-app install` ([`install`]): lays down an install from the
//!   binary that runs it.
//!
//! ON or OFF is an answer on disk: the update machine owns this copy when
//! `state.json` exists AND the running executable is the installed one
//! ([`Layout::owns`]). On macOS a bundle running from the install dir with
//! no state describing it adopts itself at its start ([`install::adopt`]).
//! `cargo run`, a bare binary out of the tarball, and a copy launched from
//! anywhere else (a translocated one too) run with the updater off and
//! touch nothing. Of the processes an install runs at once, one is its
//! updater: the one holding `<updates>/lock` ([`hold_install`]).
//!
//! ```text
//! Linux  (<data> = $XDG_DATA_HOME, else ~/.local/share):
//!   <data>/ducktape/releases/<sha>/ducktape-app
//!   <data>/ducktape/releases/<sha>.partial        a resumable download
//!   <data>/ducktape/current  -> releases/<sha>    the install path, flipped by rename(2)
//!   <data>/ducktape/previous -> releases/<sha>
//!   <data>/applications/dev.ducktape.app.desktop  Exec= current/ducktape-app
//! macOS  (<install> = $DUCKTAPE_INSTALL_DIR, else /Applications):
//!   <install>/Ducktape.app                        the CURRENT release, whole
//!   <updates>/releases/<sha>/Ducktape.app         previous and staged releases
//!   <updates>/releases/<sha>.partial
//!   <updates>/previous -> releases/<sha>
//!   <updates>/swap.json                           the bundle swap's journal
//! both   (<updates> = <config>/ducktape/updates, the app's config dir):
//!   <updates>/state.json                          the machine's phase
//!   <updates>/channel                             the channel picked (stable when absent)
//!   <updates>/lock                                held by the install's one updater
//!   <updates>/seen.json                           the highest sequence each channel has shown
//!   <updates>/keys/successor.json                 a key rotation this install saw
//! ```
//! Every path is absolute under those roots, never relative to the running
//! executable.

mod boot;
mod channel;
mod install;
mod macos;
mod stage;
#[cfg(test)]
mod tests;

pub(crate) use app_update::layout::Channel;
pub(crate) use boot::boot;
#[cfg(test)]
pub(crate) use channel::{Shown, show_here};
pub(crate) use channel::{choose, on_moved, rendered, restart, shown};
pub(crate) use install::install_cli;

use std::fmt;
use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use app_update::{Phase, Sha, state};

/// The log target every update event is under.
const TARGET: &str = "ducktape::update";
const APP_EXE: &str = "ducktape-app";
const BUNDLE: &str = "Ducktape.app";
const PARTIAL_SUFFIX: &str = ".partial";

/// The sequence this binary is published at: its commit's committer time
/// (`build.rs`), 0 for a build without git. An install pins it, so a channel
/// publishing this same commit reads as what already runs, and only a newer
/// commit, on either channel, is downloaded.
fn own_sequence() -> u64 {
    option_env!("DUCKTAPE_APP_COMMIT_TIME")
        .and_then(|sequence| sequence.parse().ok())
        .unwrap_or(0)
}

/// Where everything is. Built from the process env once ([`Layout::host`]);
/// a test builds one over a scratch root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    /// `<config>/ducktape/updates`.
    pub(crate) updates: PathBuf,
    /// Linux: `<data>/ducktape`, holding `releases/`, `current`, `previous`.
    /// macOS: the directory holding the installed `Ducktape.app`.
    pub(crate) install: PathBuf,
    /// Linux: `<data>`, for the desktop entry and its icon.
    pub(crate) data_home: PathBuf,
}

impl Layout {
    /// This user's layout, or `None` with no home to put it in.
    pub(crate) fn host() -> Option<Layout> {
        let updates = crate::backend::config_dir().ok()?.join("updates");
        let data = crate::backend::data_dir().ok()?;
        let data_home = data.parent()?.to_path_buf();
        let install = match cfg!(target_os = "macos") {
            true => std::env::var_os("DUCKTAPE_INSTALL_DIR")
                .filter(|dir| !dir.is_empty())
                .map_or_else(|| PathBuf::from("/Applications"), PathBuf::from),
            false => data,
        };
        Some(Layout {
            updates,
            install,
            data_home,
        })
    }

    pub(crate) fn state_path(&self) -> PathBuf {
        self.updates.join("state.json")
    }

    /// The channel this device follows, a device setting kept with the
    /// update state: Settings writes it, nothing else does.
    fn channel_path(&self) -> PathBuf {
        self.updates.join("channel")
    }

    /// The replay guard's marks ([`channel`]'s `against_seen`).
    fn seen_path(&self) -> PathBuf {
        self.updates.join("seen.json")
    }

    fn successor_path(&self) -> PathBuf {
        self.updates.join("keys").join("successor.json")
    }

    /// macOS: what a bundle swap wrote before `RENAME_SWAP`.
    fn journal_path(&self) -> PathBuf {
        self.updates.join("swap.json")
    }

    pub(crate) fn releases_dir(&self) -> PathBuf {
        match cfg!(target_os = "macos") {
            true => self.updates.join("releases"),
            false => self.install.join("releases"),
        }
    }

    pub(crate) fn release_dir(&self, sha: Sha) -> PathBuf {
        self.releases_dir().join(sha.to_string())
    }

    fn partial_path(&self, sha: Sha) -> PathBuf {
        self.releases_dir().join(format!("{sha}{PARTIAL_SUFFIX}"))
    }

    /// A release's executable, where it is staged.
    pub(crate) fn release_exe(&self, sha: Sha) -> PathBuf {
        match cfg!(target_os = "macos") {
            true => bundle_exe(&self.staged_bundle(sha)),
            false => self.release_dir(sha).join(APP_EXE),
        }
    }

    /// macOS: `<updates>/releases/<sha>/Ducktape.app`.
    fn staged_bundle(&self, sha: Sha) -> PathBuf {
        self.release_dir(sha).join(BUNDLE)
    }

    /// macOS: the installed bundle, which IS the current release.
    fn installed_bundle(&self) -> PathBuf {
        self.install.join(BUNDLE)
    }

    /// Linux: the install path, a symlink into `releases/`.
    pub(crate) fn current_link(&self) -> PathBuf {
        self.install.join("current")
    }

    pub(crate) fn previous_link(&self) -> PathBuf {
        match cfg!(target_os = "macos") {
            true => self.updates.join("previous"),
            false => self.install.join("previous"),
        }
    }

    /// What `current`/`previous` point at: relative, so the root can move.
    pub(crate) fn link_target(sha: Sha) -> PathBuf {
        PathBuf::from("releases").join(sha.to_string())
    }

    /// The executable the install path runs: `current/ducktape-app`, or the
    /// installed bundle's.
    pub(crate) fn app_exe(&self) -> PathBuf {
        match cfg!(target_os = "macos") {
            true => bundle_exe(&self.installed_bundle()),
            false => self.current_link().join(APP_EXE),
        }
    }

    /// Whether `exe` is this install's: under `releases/` (Linux) or inside
    /// the installed bundle (macOS). Anything else — `target/debug`, a bare
    /// binary, a translocated copy — is not, and runs with the updater off.
    pub(crate) fn owns(&self, exe: &Path) -> bool {
        let root = match cfg!(target_os = "macos") {
            true => self.installed_bundle(),
            false => self.releases_dir(),
        };
        let (Ok(exe), Ok(root)) = (exe.canonicalize(), root.canonicalize()) else {
            return false;
        };
        exe.starts_with(root)
    }

    /// Linux: the desktop entry that points the session at `current`.
    fn desktop_entry(&self) -> PathBuf {
        self.data_home
            .join("applications")
            .join("dev.ducktape.app.desktop")
    }

    /// Linux: the icon the desktop entry names (`Icon=ducktape`).
    fn icon(&self) -> PathBuf {
        self.data_home
            .join("icons/hicolor/scalable/apps/ducktape.svg")
    }
}

/// `Contents/MacOS/ducktape-app` of a bundle.
fn bundle_exe(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("MacOS").join(APP_EXE)
}

/// Why an update step stopped short: a stable snake_case `reason` (the log
/// field a grep counts) and a human `detail`. Never argv (it may carry a
/// `duck://` token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) reason: &'static str,
    pub(crate) detail: String,
}

impl Refusal {
    fn new(reason: &'static str, detail: impl Into<String>) -> Self {
        Refusal {
            reason,
            detail: detail.into(),
        }
    }

    /// An I/O error at a named path.
    fn io(reason: &'static str, path: &Path, error: &std::io::Error) -> Self {
        Refusal::new(reason, format!("{}: {error}", path.display()))
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.detail, self.reason)
    }
}

// ---- the few writers and probes every path goes through -------------------
//
// Symlink policy: nothing here follows a symlink it did not write.
// `state.json`, the journal, every `releases/<sha>` and the executables in it
// are opened only after `symlink_metadata` says they are not links;
// `current`/`previous` are the only links, by construction, and their
// targets are checked against the exact `releases/<sha>` they must name.

/// Fail on a symlink at `path` itself; an absent path is fine.
fn refuse_symlink(path: &Path) -> Result<(), Refusal> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Refusal::new(
            "symlink_refused",
            format!("{} is a symlink", path.display()),
        )),
        _ => Ok(()),
    }
}

/// What `state.json` holds; `None` when there is no file, an error for a
/// link or unparsable contents.
fn read_state(path: &Path) -> Result<Option<Phase>, Refusal> {
    refuse_symlink(path)?;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Refusal::io("state_unreadable", path, &error)),
    };
    state::decode(&text)
        .map(Some)
        .map_err(|error| Refusal::new("state_invalid", format!("{}: {error}", path.display())))
}

/// Write a file whole: tmp beside it, fsync, rename over.
fn persist(path: &Path, bytes: &[u8]) -> Result<(), Refusal> {
    refuse_symlink(path)?;
    let parent = path.parent().ok_or_else(|| {
        Refusal::new(
            "persist_failed",
            format!("{} has no parent", path.display()),
        )
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|error| Refusal::io("persist_failed", parent, &error))?;
    let tmp = tmp_name(path);
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        Refusal::io("persist_failed", path, &error)
    })
}

fn persist_phase(layout: &Layout, phase: &Phase) -> Result<(), Refusal> {
    persist(&layout.state_path(), state::encode(phase).as_bytes())
}

/// `link -> target`, atomically: a symlink at a temp name, renamed over.
fn replace_symlink(link: &Path, target: &Path) -> Result<(), Refusal> {
    let tmp = tmp_name(link);
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)
        .and_then(|()| std::fs::rename(&tmp, link))
        .map_err(|error| {
            let _ = std::fs::remove_file(&tmp);
            Refusal::io("symlink_failed", link, &error)
        })
}

/// Where a link points, or `None` when there is none; a non-link is a
/// refusal (the install path is a link by construction).
fn read_link(path: &Path) -> Result<Option<PathBuf>, Refusal> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Refusal::io("install_path_unreadable", path, &error)),
        Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(path)
            .map(Some)
            .map_err(|error| Refusal::io("install_path_unreadable", path, &error)),
        Ok(_) => Err(Refusal::new(
            "install_path_not_a_link",
            format!("{} exists and is not a symlink", path.display()),
        )),
    }
}

/// sha256 of a file's bytes, streamed.
fn digest_file(path: &Path) -> Result<Sha, Refusal> {
    use sha2::Digest as _;
    refuse_symlink(path)?;
    let mut file =
        std::fs::File::open(path).map_err(|error| Refusal::io("digest_failed", path, &error))?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|error| Refusal::io("digest_failed", path, &error))?;
    Ok(Sha::from_bytes(hasher.finalize().into()))
}

/// A regular, executable, non-link file.
fn require_executable(path: &Path) -> Result<(), Refusal> {
    refuse_symlink(path)?;
    let executable = std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    match executable {
        true => Ok(()),
        false => Err(Refusal::new(
            "executable_missing",
            format!("{} is not an executable file", path.display()),
        )),
    }
}

/// A release dir the boot step may flip to: not a link, its executable in
/// place. Sealing it is the app's job; the flip only checks it is whole.
fn require_release(layout: &Layout, sha: Sha) -> Result<(), Refusal> {
    refuse_symlink(&layout.release_dir(sha))?;
    require_executable(&layout.release_exe(sha))
}

/// A file's `(device, inode)`: what tells the image that runs from the one
/// the install path names, whatever path either was reached by.
fn file_id(path: &Path) -> Result<(u64, u64), Refusal> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path)
        .map(|meta| (meta.dev(), meta.ino()))
        .map_err(|error| Refusal::io("executable_missing", path, &error))
}

/// Whether this user may create and rename entries in `dir`.
fn require_writable(dir: &Path) -> Result<(), Refusal> {
    let writable = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())
        // SAFETY: a NUL-terminated path and a flag constant; access(2)
        // reads nothing else.
        .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK | libc::X_OK) } == 0);
    match writable {
        true => Ok(()),
        false => Err(Refusal::new(
            "install_root_not_writable",
            format!(
                "{} is not writable by this user; the app installs and updates there without root: fix its permissions{}",
                dir.display(),
                match cfg!(target_os = "macos") {
                    true => " or set DUCKTAPE_INSTALL_DIR",
                    false => "",
                }
            ),
        )),
    }
}

/// Remove a file, link or tree at `path`; nothing there is fine.
fn remove_leftover(path: &Path) -> Result<(), Refusal> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.is_dir() {
        stage::unseal(path);
    }
    let removed = match meta.is_dir() {
        true => std::fs::remove_dir_all(path),
        false => std::fs::remove_file(path),
    };
    removed.map_err(|error| Refusal::io("leftover_not_removed", path, &error))
}

/// One updater per install. Two processes of one install (on Linux every
/// `duck://` link starts one) would each step the machine from what they
/// read at their own start: the second flips while the first runs, and the
/// first then writes back a phase that names the release it runs and
/// collects the one `current` names. So the process holding
/// `<updates>/lock` is the install's one updater, and `install` its one
/// writer; a start that finds it held runs with the updater off and touches
/// nothing. `true` once this process holds it, for the rest of its life:
/// exit and `exec` let it go.
fn hold_install(layout: &Layout) -> Result<bool, Refusal> {
    static HELD: Mutex<Vec<(PathBuf, std::fs::File)>> = Mutex::new(Vec::new());
    let path = layout.updates.join("lock");
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    if held.iter().any(|(at, _)| *at == path) {
        return Ok(true);
    }
    refuse_symlink(&path)?;
    std::fs::create_dir_all(&layout.updates)
        .map_err(|error| Refusal::io("lock_failed", &layout.updates, &error))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| Refusal::io("lock_failed", &path, &error))?;
    // SAFETY: an open descriptor and flag constants.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        return match error.kind() {
            std::io::ErrorKind::WouldBlock => Ok(false),
            _ => Err(Refusal::io("lock_failed", &path, &error)),
        };
    }
    held.push((path, file));
    Ok(true)
}

/// rename(2), or a copy and remove across volumes.
fn move_tree(from: &Path, to: &Path) -> Result<(), Refusal> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
            macos::copy_bundle(from, to)?;
            remove_leftover(from)
        }
        Err(error) => Err(Refusal::io("move_failed", to, &error)),
    }
}

/// A per-process temp name beside `path`.
fn tmp_name(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn layout() -> Layout {
        Layout {
            updates: "/s/cfg/ducktape/updates".into(),
            install: "/s/data/ducktape".into(),
            data_home: "/s/data".into(),
        }
    }

    /// The tree every reader takes from one place, written out in full.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_tree_is_spelled_once() {
        let layout = layout();
        let sha = Sha::digest(b"r");
        assert_eq!(
            layout.state_path(),
            PathBuf::from("/s/cfg/ducktape/updates/state.json")
        );
        assert_eq!(
            layout.release_exe(sha),
            PathBuf::from(format!("/s/data/ducktape/releases/{sha}/ducktape-app"))
        );
        assert_eq!(
            layout.partial_path(sha),
            PathBuf::from(format!("/s/data/ducktape/releases/{sha}.partial"))
        );
        assert_eq!(
            layout.app_exe(),
            PathBuf::from("/s/data/ducktape/current/ducktape-app")
        );
        assert_eq!(
            layout.previous_link(),
            PathBuf::from("/s/data/ducktape/previous")
        );
        assert_eq!(
            layout.desktop_entry(),
            PathBuf::from("/s/data/applications/dev.ducktape.app.desktop")
        );
        assert_eq!(
            layout.successor_path(),
            PathBuf::from("/s/cfg/ducktape/updates/keys/successor.json")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_tree_keeps_releases_beside_the_state() {
        let layout = Layout {
            install: "/s/apps".into(),
            ..layout()
        };
        let sha = Sha::digest(b"r");
        assert_eq!(
            layout.release_exe(sha),
            PathBuf::from(format!(
                "/s/cfg/ducktape/updates/releases/{sha}/Ducktape.app/Contents/MacOS/ducktape-app"
            ))
        );
        assert_eq!(
            layout.app_exe(),
            PathBuf::from("/s/apps/Ducktape.app/Contents/MacOS/ducktape-app")
        );
        assert_eq!(
            layout.previous_link(),
            PathBuf::from("/s/cfg/ducktape/updates/previous")
        );
    }

    #[test]
    fn persist_writes_whole_and_read_state_refuses_a_link() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("updates").join("state.json");
        assert_eq!(read_state(&path).unwrap(), None);
        let phase = Phase::Idle(app_update::Idle {
            current: Sha::digest(b"a"),
            previous: None,
            pinned_sequence: 3,
        });
        persist(&path, state::encode(&phase).as_bytes()).unwrap();
        assert_eq!(read_state(&path).unwrap(), Some(phase));
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        let link = root.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(read_state(&link).unwrap_err().reason, "symlink_refused");
        assert_eq!(persist(&link, b"{}").unwrap_err().reason, "symlink_refused");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_state(&path).unwrap_err().reason, "state_invalid");
    }

    #[test]
    fn replace_symlink_swaps_atomically_and_read_link_names_the_target() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("current");
        assert_eq!(read_link(&link).unwrap(), None);
        replace_symlink(&link, Path::new("releases/a")).unwrap();
        replace_symlink(&link, Path::new("releases/b")).unwrap();
        assert_eq!(read_link(&link).unwrap(), Some(PathBuf::from("releases/b")));
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            1,
            "no temp left behind"
        );
        std::fs::create_dir(root.path().join("dir")).unwrap();
        assert_eq!(
            read_link(&root.path().join("dir")).unwrap_err().reason,
            "install_path_not_a_link"
        );
    }

    /// The updater is on only for the install's own executable: a binary
    /// anywhere else — `target/debug`, the tarball's — is not it, even with
    /// a `state.json` on disk.
    #[cfg(target_os = "linux")]
    #[test]
    fn only_an_executable_inside_the_install_is_owned() {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout {
            updates: root.path().join("cfg/updates"),
            install: root.path().join("data/ducktape"),
            data_home: root.path().join("data"),
        };
        let sha = Sha::digest(b"a");
        let inside = layout.release_exe(sha);
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, b"app").unwrap();
        let outside = root.path().join("target/debug/ducktape-app");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, b"app").unwrap();
        assert!(layout.owns(&inside));
        assert!(!layout.owns(&outside));
        assert!(!layout.owns(&root.path().join("missing")));
    }
}
