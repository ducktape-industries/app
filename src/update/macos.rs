//! The macOS side: a release is the whole `Ducktape.app`, so a flip swaps
//! bundles, a copy keeps a bundle whole, and a staged bundle's code signature
//! is checked before it may run.
//!
//! The installed `<install>/Ducktape.app` is swapped whole against the staged
//! one with `renamex_np(RENAME_SWAP)` (atomic, same volume), and the old
//! bundle — now sitting at the staged path — is moved to
//! `releases/<from>/Ducktape.app`, which `previous` then names. Both paths
//! hold a bundle before AND after the swap, so the flip first records the
//! incoming executable's digest in `<updates>/swap.json`; a boot in
//! `Swapping` hashes the installed executable against it to learn whether the
//! swap landed, then finishes the bookkeeping.
//!
//! Everything here compiles on every platform so the Linux build type-checks
//! it; only the swap call itself is macOS's, and the boot step reaches this
//! module on macOS alone.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use app_update::{Sha, SwapState};

use super::{Layout, Refusal};

/// `codesign --verify --deep --strict`: the bundle's code is whole. The
/// signed manifest and the archive digest establish release trust; this is
/// the integrity check on what will run, not a Gatekeeper assessment.
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

/// A whole bundle, copied with `/usr/bin/ditto`: the one copier that keeps
/// it whole (symlinks, modes, extended attributes, the code signature). The
/// quarantine bit is then removed from this managed copy only.
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
    // recursive (ditto keeps nested attributes, and a bundle whose inner
    // files keep the bit is still quarantined), links not followed. Its
    // exit is not read: xattr fails for every file that had no such
    // attribute, which is the clean case.
    let _ = Command::new("/usr/bin/xattr")
        .args(["-r", "-s", "-d", "com.apple.quarantine"])
        .arg(to)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

/// What a bundle swap wrote before `RENAME_SWAP`: which release is coming
/// in, the digest of its executable, and where the outgoing bundle sits
/// right after the swap.
#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct Journal {
    to: Sha,
    app_digest: Sha,
    swapped_with: PathBuf,
}

/// Swap the installed bundle against the staged `to` and park the old one
/// under `releases/<from>`.
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
    // the app seals a staged release `a-w`; the swap renames entries inside
    // `releases/<to>/`, which needs write on that directory
    unseal_dir(&layout.release_dir(to))?;
    let source = same_volume_copy(&staged, &layout.install, to)?;
    // moving a directory to another parent rewrites its `..`, which needs
    // write on the directory itself: RENAME_SWAP moves both bundles, and
    // `finish` moves the outgoing one again, to its park slot
    unseal_dir(&source)?;
    unseal_dir(&installed)?;
    let journal = Journal {
        to,
        app_digest: super::digest_file(&super::bundle_exe(&source))?,
        swapped_with: source.clone(),
    };
    let text = serde_json::to_string_pretty(&journal).expect("a Journal serializes");
    super::persist(&layout.journal_path(), text.as_bytes())?;
    rename_swap(&installed, &source)?;
    finish(layout, from, to, &journal)
}

/// After the swap landed: park the outgoing bundle, point `previous` at
/// it, drop the journal. Idempotent, so a boot that finds a journal can
/// call it again.
fn finish(layout: &Layout, from: Sha, to: Sha, journal: &Journal) -> Result<(), Refusal> {
    let park = layout.staged_bundle(from);
    let old_still_at_swap_path = std::fs::symlink_metadata(&journal.swapped_with).is_ok();
    if old_still_at_swap_path {
        let parent = layout.release_dir(from);
        std::fs::create_dir_all(&parent)
            .map_err(|error| Refusal::io("park_failed", &parent, &error))?;
        super::move_tree(&journal.swapped_with, &park)?;
    }
    let _ = std::fs::remove_dir(layout.release_dir(to));
    super::replace_symlink(&layout.previous_link(), &Layout::link_target(from))?;
    let path = layout.journal_path();
    std::fs::remove_file(&path).map_err(|error| Refusal::io("journal_unremovable", &path, &error))
}

/// Which side of a crashed swap is live: no journal is a swap that never
/// started; a journal whose digest the installed executable has is one
/// that landed (and is finished now).
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
            finish(layout, from, to, &journal)?;
            Ok(SwapState::Landed)
        }
        false => {
            std::fs::remove_file(&path)
                .map_err(|error| Refusal::io("journal_unremovable", &path, &error))?;
            Ok(SwapState::Untouched)
        }
    }
}

/// The staged bundle, on the install dir's volume: itself when it already
/// is, else a copy at `<install>/.Ducktape.app.<sha>`.
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

/// `u+w` on one directory (not its contents).
fn unseal_dir(dir: &Path) -> Result<(), Refusal> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(dir)
        .map_err(|error| Refusal::io("release_missing", dir, &error))?
        .permissions()
        .mode();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode | 0o200))
        .map_err(|error| Refusal::io("unseal_failed", dir, &error))
}

/// `renamex_np(a, b, RENAME_SWAP)`: both paths exist before and after;
/// their contents trade places atomically.
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

    fn chmod(mode: &str, path: &Path) {
        assert!(
            Command::new("chmod")
                .args(["-R", mode])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }

    /// The app seals a staged release `chmod -R a-w`; the swap, the park
    /// and the swap back must still land, and leave no journal.
    #[test]
    fn sealed_bundles_swap_in_park_and_swap_back() {
        let root = tempfile::tempdir().unwrap();
        let layout = layout(root.path());
        let (a, b) = (Sha::digest(b"a"), Sha::digest(b"b"));
        bundle(&layout.installed_bundle(), b"app-a");
        bundle(&layout.staged_bundle(b), b"app-b");
        chmod("a-w", &layout.release_dir(b));
        chmod("a-w", &layout.installed_bundle());

        let flipped = swap_bundles(&layout, a, b);
        let rolled_back = flipped.clone().and_then(|()| swap_bundles(&layout, b, a));
        let parked_b =
            super::super::digest_file(&super::super::bundle_exe(&layout.staged_bundle(b)));
        let installed = super::super::digest_file(&layout.app_exe());
        chmod("u+w", root.path());
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

    /// A crash after the journal and before the swap reads as untouched; a
    /// crash after the swap reads as landed and is finished.
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
