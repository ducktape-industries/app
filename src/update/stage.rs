//! Staging a downloaded release on disk: the archive out of
//! `releases/<sha>.partial` into `releases/<sha>/` (the FINAL directory, never
//! a temp elsewhere — what is verified is what is flipped), the bundle check
//! that makes it trusted, the seal that keeps it that way, and the collector
//! that removes what no phase names any more.
//!
//! The archive (`.tar.gz`) holds the release directory's contents at its top
//! level: Linux `ducktape-app`; macOS `Ducktape.app/…`. Extraction accepts
//! regular files, directories and symlinks whose target stays under the
//! release directory; it refuses absolute paths, `..`, a symlink that would
//! leave the directory, and hard links. Every refusal is a stable snake_case
//! token: it is the log's `reason` and what Settings reads.

use std::path::{Component, Path, PathBuf};

use app_update::Sha;
use tracing::{info, warn};

use super::{APP_EXE, BUNDLE, PARTIAL_SUFFIX, TARGET};

/// `Verify`: hash the archive again, extract it into `release_dir`, check
/// the release is whole (and, on macOS, that its bundle signature holds).
/// `release_dir` is replaced whole if something is already there — a dir at
/// a sha the machine has not verified is a leftover, never a release.
pub(super) fn stage(archive: &Path, sha: Sha, release_dir: &Path) -> Result<(), String> {
    let landed = super::digest_file(archive).map_err(|refusal| refusal.reason.to_string())?;
    if landed != sha {
        return Err("sha256_mismatch".into());
    }
    clear_release_dir(release_dir)?;
    extract(archive, release_dir)?;
    let exe = match cfg!(target_os = "macos") {
        true => super::bundle_exe(&release_dir.join(BUNDLE)),
        false => release_dir.join(APP_EXE),
    };
    if !exe.is_file() {
        return Err("release_incomplete".into());
    }
    if cfg!(target_os = "macos") {
        super::macos::verify_bundle(&release_dir.join(BUNDLE)).map_err(|refusal| refusal.reason)?;
    }
    Ok(())
}

/// A release dir is a real directory the app creates: a link at its path
/// is refused, a leftover directory is unsealed and removed.
fn clear_release_dir(release_dir: &Path) -> Result<(), String> {
    let Ok(meta) = std::fs::symlink_metadata(release_dir) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        return Err("release_dir_is_a_link".into());
    }
    unseal(release_dir);
    std::fs::remove_dir_all(release_dir).map_err(|_| "io".to_string())
}

fn extract(archive: &Path, release_dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|_| "io")?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    tar.set_preserve_permissions(false);
    tar.set_preserve_ownerships(false);
    std::fs::create_dir_all(release_dir).map_err(|_| "io")?;
    let entries = tar.entries().map_err(|_| "archive_unreadable")?;
    for entry in entries {
        let mut entry = entry.map_err(|_| "archive_unreadable")?;
        let entry_path = entry.path().map_err(|_| "archive_unreadable")?.into_owned();
        let destination = entry_destination(release_dir, &entry_path)?;
        admit(
            entry.header().entry_type(),
            &destination,
            release_dir,
            &entry,
        )?;
        entry.unpack(&destination).map_err(|_| "extract_failed")?;
    }
    Ok(())
}

/// Whether one entry, its path already inside the release dir, is a kind
/// a release is made of — and, for a symlink, one that stays inside.
fn admit<R: std::io::Read>(
    kind: tar::EntryType,
    destination: &Path,
    release_dir: &Path,
    entry: &tar::Entry<'_, R>,
) -> Result<(), String> {
    match kind {
        tar::EntryType::Regular | tar::EntryType::Directory => Ok(()),
        tar::EntryType::Symlink => {
            let target = entry
                .link_name()
                .map_err(|_| "archive_unreadable")?
                .ok_or("symlink_without_target")?;
            match symlink_stays_inside(release_dir, destination, &target) {
                true => Ok(()),
                false => Err("symlink_escapes".into()),
            }
        }
        tar::EntryType::Link => Err("hard_link".into()),
        // devices, fifos, sparse and extension headers: none is a release file
        _ => Err("unsupported_entry".into()),
    }
}

/// Where an entry lands: under `root`, through normal components only.
fn entry_destination(root: &Path, entry_path: &Path) -> Result<PathBuf, String> {
    let mut destination = root.to_path_buf();
    for component in entry_path.components() {
        match component {
            Component::Normal(part) => destination.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("path_escapes".into()),
            Component::RootDir | Component::Prefix(_) => return Err("path_absolute".into()),
        }
    }
    Ok(destination)
}

/// A symlink at `link` (under `root`) pointing at `target` stays inside
/// when the target is relative and, resolved lexically from the link's
/// directory, never climbs above `root`.
fn symlink_stays_inside(root: &Path, link: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let Ok(link_relative) = link.strip_prefix(root) else {
        return false;
    };
    let mut depth: Vec<&std::ffi::OsStr> = link_relative
        .parent()
        .map(|parent| {
            parent
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => Some(part),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    for component in target.components() {
        match component {
            Component::Normal(part) => depth.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

// ---- seal / unseal ---------------------------------------------------------

/// `chmod -R a-w`: nothing in a staged release changes after it verified.
/// Symlinks are left alone (their mode is meaningless; their target is
/// inside and gets its own visit).
pub(super) fn seal(dir: &Path) -> std::io::Result<()> {
    walk(dir, &|path, mode| set_mode(path, mode & !0o222))
}

/// `chmod -R u+w`, so a sealed release can be moved or removed. Errors are
/// ignored: the remove that follows reports its own.
pub(super) fn unseal(dir: &Path) {
    let _ = walk(dir, &|path, mode| set_mode(path, mode | 0o200));
}

fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Every non-link entry under `dir`, children before their directory. A
/// sealed directory still reads (`r-x`) and chmod needs ownership, not a
/// writable parent, so one order serves both the seal and the unseal.
fn walk(dir: &Path, apply: &dyn Fn(&Path, u32) -> std::io::Result<()>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            walk(&entry?.path(), apply)?;
        }
    }
    apply(dir, meta.permissions().mode() & 0o7777)
}

// ---- gc --------------------------------------------------------------------

/// `Gc`: remove every `releases/<sha>` whose sha is not in `keep`, and every
/// `<sha>.partial` — once the machine collects, nothing is downloading, so a
/// partial is a leftover whatever its name. Only entries named by a sha are
/// touched; anything else in the directory is not this module's.
pub(super) fn collect(releases_dir: &Path, keep: &[Sha]) {
    let Ok(entries) = std::fs::read_dir(releases_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let (name, path) = (
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        );
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let partial = name
            .strip_suffix(PARTIAL_SUFFIX)
            .and_then(|stem| stem.parse::<Sha>().ok())
            .filter(|_| meta.is_file());
        let release = name
            .parse::<Sha>()
            .ok()
            .filter(|sha| meta.is_dir() && !keep.contains(sha));
        let Some(sha) = partial.or(release) else {
            continue;
        };
        let kind = if meta.is_dir() { "release" } else { "partial" };
        if meta.is_dir() {
            unseal(&path);
        }
        let removed = match meta.is_dir() {
            true => std::fs::remove_dir_all(&path),
            false => std::fs::remove_file(&path),
        };
        match removed {
            Ok(()) => info!(target: TARGET, event = "app_update_gc", %sha, kind),
            Err(error) => {
                warn!(target: TARGET, event = "app_update_gc_failed", %sha, kind, %error)
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::PermissionsExt as _;

    /// A `.tar.gz` from a list of entries.
    pub(in crate::update) enum Item {
        File(&'static str, Vec<u8>, u32),
        Dir(&'static str),
        Symlink(&'static str, &'static str),
        HardLink(&'static str, &'static str),
    }

    pub(in crate::update) fn archive_of(items: &[Item]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for item in items {
            let mut header = tar::Header::new_gnu();
            match item {
                Item::File(path, bytes, mode) => {
                    header.set_size(bytes.len() as u64);
                    header.set_mode(*mode);
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, path, bytes.as_slice())
                        .unwrap();
                }
                Item::Dir(path) => {
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, path, std::io::empty())
                        .unwrap();
                }
                Item::Symlink(path, target) => {
                    header.set_size(0);
                    header.set_mode(0o777);
                    header.set_entry_type(tar::EntryType::Symlink);
                    builder.append_link(&mut header, path, target).unwrap();
                }
                Item::HardLink(path, target) => {
                    header.set_size(0);
                    header.set_mode(0o644);
                    header.set_entry_type(tar::EntryType::Link);
                    builder.append_link(&mut header, path, target).unwrap();
                }
            }
        }
        gzip(&builder.into_inner().unwrap())
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    /// A release tree with a nested directory and links that stay inside.
    pub(in crate::update) fn whole_release(app: &[u8]) -> Vec<Item> {
        vec![
            Item::File(APP_EXE, app.to_vec(), 0o755),
            Item::Dir("res"),
            Item::File("res/icon.svg", b"svg".to_vec(), 0o644),
            Item::Symlink("res-alias", "res"),
            Item::Symlink("res/back", "../res/icon.svg"),
        ]
    }

    /// A raw entry the `tar` builder itself refuses to write (`..`, a
    /// leading `/`): the header's name bytes set directly.
    fn raw_entry(name: &[u8]) -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        let mut builder = tar::Builder::new(Vec::new());
        builder.append(&header, &b"x"[..]).unwrap();
        gzip(&builder.into_inner().unwrap())
    }

    fn staged_raw(name: &[u8]) -> Result<(), String> {
        let bytes = raw_entry(name);
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("raw.partial");
        std::fs::write(&archive, &bytes).unwrap();
        stage(&archive, Sha::digest(&bytes), &root.path().join("release"))
    }

    fn staged(items: &[Item]) -> (tempfile::TempDir, Result<(), String>) {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive_of(items);
        let sha = Sha::digest(&bytes);
        let archive = root.path().join(format!("{sha}.partial"));
        std::fs::write(&archive, &bytes).unwrap();
        let outcome = stage(&archive, sha, &root.path().join("release"));
        (root, outcome)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_whole_archive_extracts_with_its_modes_and_inside_links() {
        let (root, outcome) = staged(&whole_release(b"app"));
        assert_eq!(outcome, Ok(()));
        let release = root.path().join("release");
        let app_mode = std::fs::metadata(release.join(APP_EXE))
            .unwrap()
            .permissions()
            .mode()
            & 0o111;
        assert_eq!(app_mode, 0o111, "the executable bit survives");
        assert_eq!(std::fs::read(release.join("res/icon.svg")).unwrap(), b"svg");
        assert_eq!(
            std::fs::read_link(release.join("res-alias")).unwrap(),
            PathBuf::from("res")
        );
        assert_eq!(std::fs::read(release.join("res/back")).unwrap(), b"svg");
    }

    #[test]
    fn the_archive_hash_is_checked_again_before_anything_lands() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive_of(&whole_release(b"app"));
        let archive = root.path().join("x.partial");
        std::fs::write(&archive, &bytes).unwrap();
        let outcome = stage(
            &archive,
            Sha::digest(b"other"),
            &root.path().join("release"),
        );
        assert_eq!(outcome, Err("sha256_mismatch".into()));
        assert!(!root.path().join("release").exists());
    }

    #[test]
    fn a_parent_component_is_refused() {
        assert_eq!(staged_raw(b"../escape"), Err("path_escapes".into()));
        assert_eq!(staged_raw(b"res/../../escape"), Err("path_escapes".into()));
    }

    #[test]
    fn an_absolute_path_is_refused() {
        assert_eq!(staged_raw(b"/etc/passwd"), Err("path_absolute".into()));
    }

    #[test]
    fn a_symlink_leaving_the_release_dir_is_refused() {
        let mut items = whole_release(b"app");
        items.push(Item::Symlink("res/out", "../../secret"));
        let (_root, outcome) = staged(&items);
        assert_eq!(outcome, Err("symlink_escapes".into()));

        let mut items = whole_release(b"app");
        items.push(Item::Symlink("abs", "/etc"));
        let (_root, outcome) = staged(&items);
        assert_eq!(outcome, Err("symlink_escapes".into()));
    }

    #[test]
    fn a_hard_link_is_refused() {
        let mut items = whole_release(b"app");
        items.push(Item::HardLink("twin", APP_EXE));
        let (_root, outcome) = staged(&items);
        assert_eq!(outcome, Err("hard_link".into()));
    }

    #[test]
    fn an_archive_without_the_app_is_incomplete() {
        let (_root, outcome) = staged(&[Item::File("readme", b"x".to_vec(), 0o644)]);
        assert_eq!(outcome, Err("release_incomplete".into()));
    }

    #[test]
    fn bytes_that_are_not_a_gzip_tar_are_unreadable() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("x.partial");
        std::fs::write(&archive, b"not a release").unwrap();
        let outcome = stage(
            &archive,
            Sha::digest(b"not a release"),
            &root.path().join("release"),
        );
        assert_eq!(outcome, Err("archive_unreadable".into()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_leftover_dir_is_replaced_and_a_link_at_its_path_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let bytes = archive_of(&whole_release(b"app"));
        let sha = Sha::digest(&bytes);
        let archive = root.path().join(format!("{sha}.partial"));
        std::fs::write(&archive, &bytes).unwrap();
        let release = root.path().join("release");
        std::fs::create_dir_all(release.join("stale")).unwrap();
        std::fs::write(release.join("stale/old"), b"old").unwrap();
        seal(&release).unwrap();
        assert_eq!(stage(&archive, sha, &release), Ok(()));
        assert!(!release.join("stale").exists(), "the leftover went");

        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(&release, &linked).unwrap();
        assert_eq!(
            stage(&archive, sha, &linked),
            Err("release_dir_is_a_link".into())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn seal_takes_every_write_bit_and_unseal_gives_the_owner_one_back() {
        let (root, outcome) = staged(&whole_release(b"app"));
        assert_eq!(outcome, Ok(()));
        let release = root.path().join("release");
        seal(&release).unwrap();
        for path in [
            release.clone(),
            release.join(APP_EXE),
            release.join("res"),
            release.join("res/icon.svg"),
        ] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o222, 0, "{} is writable", path.display());
        }
        assert!(
            std::fs::write(release.join("res/new"), b"x").is_err(),
            "a sealed dir takes no new entry"
        );
        unseal(&release);
        for path in [release.join(APP_EXE), release.join("res")] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o200, 0o200, "{} not reopened", path.display());
        }
    }

    #[test]
    fn collect_removes_only_unkept_shas_and_every_partial() {
        let root = tempfile::tempdir().unwrap();
        let releases = root.path().join("releases");
        let keep = Sha::digest(b"keep");
        let drop = Sha::digest(b"drop");
        for sha in [keep, drop] {
            let dir = releases.join(sha.to_string());
            std::fs::create_dir_all(dir.join("res")).unwrap();
            std::fs::write(dir.join(APP_EXE), b"app").unwrap();
            seal(&dir).unwrap();
        }
        std::fs::create_dir_all(releases.join("not-a-sha")).unwrap();
        std::fs::write(releases.join(format!("{keep}.partial")), b"p").unwrap();
        std::fs::write(releases.join(format!("{drop}.partial")), b"p").unwrap();
        std::fs::write(releases.join("state.json"), b"{}").unwrap();

        collect(&releases, &[keep]);

        assert!(releases.join(keep.to_string()).join(APP_EXE).is_file());
        assert!(
            !releases.join(drop.to_string()).exists(),
            "the unkept release went"
        );
        assert!(
            releases.join("not-a-sha").is_dir(),
            "a stranger dir is untouched"
        );
        assert!(!releases.join(format!("{keep}.partial")).exists());
        assert!(!releases.join(format!("{drop}.partial")).exists());
        assert!(
            releases.join("state.json").is_file(),
            "a stranger file is untouched"
        );
    }

    #[test]
    fn symlink_containment_is_lexical_from_the_links_directory() {
        let root = Path::new("/r");
        let inside = |link: &str, target: &str| {
            symlink_stays_inside(root, &root.join(link), Path::new(target))
        };
        assert!(inside("a/b", "../c"));
        assert!(inside("a/b", "./c/../d"));
        assert!(inside("Contents/MacOS/views", "../Resources/views"));
        assert!(!inside("a/b", "../../c"));
        assert!(!inside("top", "../x"));
        assert!(!inside("a", "/etc"));
    }
}
