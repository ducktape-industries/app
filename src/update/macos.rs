//! The macOS side: a release is the whole `Ducktape.app`, so a flip swaps
//! bundles, a copy keeps a bundle whole, and a staged bundle's code signature
//! is checked before it may run.
//!
//! The installed `<install>/Ducktape.app` is swapped whole against the staged
//! one with `renamex_np(RENAME_SWAP)` (atomic, same volume), and the old
//! bundle — now sitting at the staged path — is moved to
//! `releases.noindex/<from>/Ducktape.app`, which `previous` then names. Both paths
//! hold a bundle before AND after the swap, so the flip first records the
//! incoming executable's digest in `<updates>/swap.json`; a boot in
//! `Swapping` hashes the installed executable against it to learn whether the
//! swap landed, then finishes the bookkeeping.
//!
//! A bundle dragged over the installed one (from a DMG, older or newer) is
//! not the release the state names, and the boot step adopts it
//! ([`describes`]). It tells by the running executable's digest: a release
//! laid down by `install` or adopted is named by that digest already; one
//! staged from an archive is named by the archive's, so its executable's is
//! recorded as it stages, in `<updates>/executables.json`.
//!
//! The installed bundle is the one app of the install LaunchServices knows.
//! It registers, within seconds, any bundle that appears where Spotlight
//! looks, and keeps the record by path after the bundle is gone; so the
//! releases dir is one Spotlight skips, the swap registers the installed
//! path again for the bundle now there, and a removal unregisters first
//! ([`launch_services`]).
//!
//! The release a flip put in place starts as a new launch of the installed
//! bundle ([`open_new`]), never by `exec` from the one that flipped.
//!
//! Everything here compiles on every platform so the Linux build type-checks
//! it; only the swap call itself is macOS's, and the boot step reaches this
//! module on macOS alone.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use std::collections::BTreeMap;
use std::ffi::OsString;

use ducktape_updater::{Phase, Sha, SwapState};

use super::{Layout, Refusal};

pub(super) fn verify_bundle(bundle: &Path) -> Result<(), Refusal> {
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| Refusal::io("codesign_unavailable", bundle, &error))?;
    match status.success() {
        true => Ok(()),
        false => Err(Refusal::new(
            "codesign_rejected",
            format!("{} failed codesign --verify ({status})", bundle.display()),
        )),
    }
}

pub(super) fn describes(layout: &Layout, phase: &Phase, own: Sha) -> bool {
    let known = executables(layout);
    let runs = |release: Sha| known.get(&release).copied().unwrap_or(release) == own;
    match phase {
        Phase::Swapping(swapping) => runs(swapping.from) || runs(swapping.to),
        phase => runs(phase.current()),
    }
}

pub(super) fn record_executable(layout: &Layout, release: Sha) -> Result<(), Refusal> {
    let mut known = executables(layout);
    known.insert(release, super::digest_file(&layout.release_exe(release))?);
    keep_executables(layout, &known)
}

pub(super) fn forget_executables(layout: &Layout, keep: &[Sha]) -> Result<(), Refusal> {
    let mut known = executables(layout);
    let before = known.len();
    known.retain(|release, _| keep.contains(release));
    match known.len() == before {
        true => Ok(()),
        false => keep_executables(layout, &known),
    }
}

fn executables(layout: &Layout) -> BTreeMap<Sha, Sha> {
    std::fs::read_to_string(layout.executables_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn keep_executables(layout: &Layout, known: &BTreeMap<Sha, Sha>) -> Result<(), Refusal> {
    let text = serde_json::to_string_pretty(known).expect("digests serialize");
    super::persist(&layout.executables_path(), text.as_bytes())
}

pub(super) fn copy_bundle(from: &Path, to: &Path) -> Result<(), Refusal> {
    if !cfg!(target_os = "macos") {
        return Err(Refusal::new(
            "bundle_copy_needs_macos",
            "a whole-bundle copy is a macOS operation",
        ));
    }
    let status = Command::new("/usr/bin/ditto")
        .arg(from)
        .arg(to)
        .status()
        .map_err(|error| Refusal::io("copy_failed", to, &error))?;
    if !status.success() {
        return Err(Refusal::new(
            "copy_failed",
            format!("ditto {} {} exited {status}", from.display(), to.display()),
        ));
    }
    let _ = Command::new("/usr/bin/xattr")
        .args(["-r", "-s", "-d", "com.apple.quarantine"])
        .arg(to)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister";

#[cfg(test)]
thread_local! {
    pub(super) static LAUNCH_SERVICES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn launch_services(bundle: &Path, registered: bool) {
    #[cfg(test)]
    if !LAUNCH_SERVICES.with(std::cell::Cell::get) {
        return;
    }
    if !cfg!(target_os = "macos") {
        return;
    }
    let flag = match registered {
        true => "-f",
        false => "-u",
    };
    let output = Command::new(LSREGISTER)
        .arg(flag)
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output();
    let said = match &output {
        Ok(output) if output.status.success() => return,
        Ok(output) => format!(
            "{}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => error.to_string(),
    };
    match registered {
        true => {
            tracing::warn!(target: super::TARGET, event = "app_update_launch_services_failed", flag, bundle = %bundle.display(), said)
        }
        false => {
            tracing::debug!(target: super::TARGET, event = "app_update_launch_services_failed", flag, bundle = %bundle.display(), said)
        }
    }
}

pub(super) fn open_new(bundle: &Path, args: &[OsString]) -> Result<(), String> {
    let mut open = Command::new("/usr/bin/open");
    open.arg("-n").arg(bundle);
    if !args.is_empty() {
        open.arg("--args").args(args);
    }
    match open.stdin(Stdio::null()).output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "{}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct Journal {
    to: Sha,
    app_digest: Sha,
    swapped_with: PathBuf,
}

pub(super) fn swap_bundles(layout: &Layout, from: Sha, to: Sha) -> Result<(), Refusal> {
    let installed = layout.installed_bundle();
    let staged = layout.staged_bundle(to);
    super::require_release(layout, to)?;
    super::require_writable(&layout.install)?;
    let park = layout.staged_bundle(from);
    if std::fs::symlink_metadata(&park).is_ok() {
        return Err(Refusal::new(
            "park_slot_occupied",
            format!("{} already exists; remove it first", park.display()),
        ));
    }
    let source = same_volume_copy(&staged, &layout.install, to)?;
    let journal = Journal {
        to,
        app_digest: super::digest_file(&super::bundle_exe(&source))?,
        swapped_with: source.clone(),
    };
    let text = serde_json::to_string_pretty(&journal).expect("a Journal serializes");
    super::persist(&layout.journal_path(), text.as_bytes())?;
    rename_swap(&installed, &source)?;
    park_outgoing_bundle_and_clear_swap_journal(layout, from, to, &journal)
}

fn park_outgoing_bundle_and_clear_swap_journal(
    layout: &Layout,
    from: Sha,
    to: Sha,
    journal: &Journal,
) -> Result<(), Refusal> {
    let park = layout.staged_bundle(from);
    let old_still_at_swap_path = std::fs::symlink_metadata(&journal.swapped_with).is_ok();
    if old_still_at_swap_path {
        let parent = layout.release_dir(from);
        std::fs::create_dir_all(&parent)
            .map_err(|error| Refusal::io("park_failed", &parent, &error))?;
        super::move_tree(&journal.swapped_with, &park)?;
    }
    launch_services(&layout.installed_bundle(), true);
    let _ = std::fs::remove_dir(layout.release_dir(to));
    super::replace_symlink(&layout.previous_link(), &Layout::link_target(from))?;
    let path = layout.journal_path();
    std::fs::remove_file(&path).map_err(|error| Refusal::io("journal_unremovable", &path, &error))
}

pub(super) fn resolve_swap(layout: &Layout, from: Sha, to: Sha) -> Result<SwapState, Refusal> {
    let path = layout.journal_path();
    super::refuse_symlink(&path)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SwapState::Untouched);
        }
        Err(error) => return Err(Refusal::io("journal_unreadable", &path, &error)),
    };
    let journal: Journal = serde_json::from_str(&text)
        .map_err(|error| Refusal::new("journal_invalid", format!("{}: {error}", path.display())))?;
    if journal.to != to {
        return Err(Refusal::new(
            "journal_mismatch",
            format!("{} names another release", path.display()),
        ));
    }
    let installed = super::digest_file(&super::bundle_exe(&layout.installed_bundle()))?;
    match installed == journal.app_digest {
        true => {
            park_outgoing_bundle_and_clear_swap_journal(layout, from, to, &journal)?;
            Ok(SwapState::Landed)
        }
        false => {
            std::fs::remove_file(&path)
                .map_err(|error| Refusal::io("journal_unremovable", &path, &error))?;
            Ok(SwapState::Untouched)
        }
    }
}

fn same_volume_copy(staged: &Path, install: &Path, sha: Sha) -> Result<PathBuf, Refusal> {
    let device = |path: &Path, reason| {
        std::fs::metadata(path)
            .map(|meta| meta.dev())
            .map_err(|error| Refusal::io(reason, path, &error))
    };
    if device(staged, "release_missing")? == device(install, "install_root_missing")? {
        return Ok(staged.to_path_buf());
    }
    let copy = install.join(format!(".Ducktape.app.{sha}"));
    super::remove_leftover(&copy)?;
    copy_bundle(staged, &copy)?;
    Ok(copy)
}

#[cfg(target_os = "macos")]
fn rename_swap(a: &Path, b: &Path) -> Result<(), Refusal> {
    let c_path = |path: &Path| {
        std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| Refusal::new("bad_path", format!("{} holds a NUL", path.display())))
    };
    let (from, to) = (c_path(a)?, c_path(b)?);
    // SAFETY: two NUL-terminated paths and a flag constant.
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) };
    match rc {
        0 => Ok(()),
        _ => Err(Refusal::io(
            "swap_failed",
            a,
            &std::io::Error::last_os_error(),
        )),
    }
}

#[cfg(not(target_os = "macos"))]
fn rename_swap(_: &Path, _: &Path) -> Result<(), Refusal> {
    Err(Refusal::new(
        "bundle_swap_needs_macos",
        "a whole-bundle swap is a macOS operation",
    ))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn layout(root: &Path) -> Layout {
        Layout {
            updates: root.join("updates"),
            install: root.join("apps"),
            data_home: root.join("data"),
        }
    }

    fn bundle(dir: &Path, app: &[u8]) {
        let exe = super::super::bundle_exe(dir);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, app).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn bundles_swap_in_park_and_swap_back() {
        let root = tempfile::tempdir().unwrap();
        let layout = layout(root.path());
        let (a, b) = (Sha::digest(b"a"), Sha::digest(b"b"));
        bundle(&layout.installed_bundle(), b"app-a");
        bundle(&layout.staged_bundle(b), b"app-b");

        let flipped = swap_bundles(&layout, a, b);
        let rolled_back = flipped.clone().and_then(|()| swap_bundles(&layout, b, a));
        let parked_b =
            super::super::digest_file(&super::super::bundle_exe(&layout.staged_bundle(b)));
        let installed = super::super::digest_file(&layout.app_exe());
        assert_eq!(flipped, Ok(()));
        assert_eq!(rolled_back, Ok(()));
        assert_eq!(installed.unwrap(), Sha::digest(b"app-a"));
        assert_eq!(parked_b.unwrap(), Sha::digest(b"app-b"));
        assert_eq!(
            std::fs::read_link(layout.previous_link()).unwrap(),
            Layout::link_target(b)
        );
        assert!(!layout.journal_path().exists());
    }

    #[test]
    fn a_journal_tells_which_side_of_a_crashed_swap_is_live() {
        let root = tempfile::tempdir().unwrap();
        let layout = layout(root.path());
        let (a, b) = (Sha::digest(b"a"), Sha::digest(b"b"));
        assert_eq!(resolve_swap(&layout, a, b), Ok(SwapState::Untouched));
        bundle(&layout.installed_bundle(), b"app-a");
        bundle(&layout.staged_bundle(b), b"app-b");
        let journal = Journal {
            to: b,
            app_digest: Sha::digest(b"app-b"),
            swapped_with: layout.staged_bundle(b),
        };
        let text = serde_json::to_string(&journal).unwrap();
        super::super::persist(&layout.journal_path(), text.as_bytes()).unwrap();
        assert_eq!(resolve_swap(&layout, a, b), Ok(SwapState::Untouched));
        assert!(!layout.journal_path().exists());

        super::super::persist(&layout.journal_path(), text.as_bytes()).unwrap();
        rename_swap(&layout.installed_bundle(), &layout.staged_bundle(b)).unwrap();
        assert_eq!(resolve_swap(&layout, a, b), Ok(SwapState::Landed));
        assert_eq!(
            super::super::digest_file(&super::super::bundle_exe(&layout.staged_bundle(a))).unwrap(),
            Sha::digest(b"app-a"),
            "the outgoing bundle is parked"
        );
        assert!(!layout.journal_path().exists());
    }
}
