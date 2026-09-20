//! `install --from <built release>`: lay down a CLEAN install. Seed
//! `releases/<sha>`, put it at the install path, write `state.json` as a
//! fresh `Idle`, and on Linux register the desktop entry that points the
//! session at the launcher. What `make install` runs; also how a dev puts a
//! local build under the launcher. `--release-key HEX` also pins the key the
//! app's release channel verifies under; without a pinned key the app fetches
//! no update.
//!
//! An install is not an update. It keeps nothing of what it found: no
//! `previous` to roll back to, no staged release, no swap journal, and the
//! state it writes knows only the release it just placed. Offering, staging,
//! flipping and rolling back are the update path's, which starts from the
//! state this leaves. So an install never refuses for what is already there —
//! running it again is how a machine gets back to a known set.
//!
//! A locally built release has no archive, so its identity is the sha256 of
//! its `ducktape-app` executable. A release already seeded under that sha
//! is reused as is. A source that carries no `ducktape-launcher` gets this
//! one copied in — a release must be able to boot itself.
//!
//! A release says which it is in `release.json` at its archive root
//! ([`ReleaseIdentity`]): its sequence becomes the pin, so the channel
//! publishing that same sequence reads as what already runs. A directory
//! without the file (a developer's build) pins sequence 0.

use std::fs as std_fs;
use std::path::{Path, PathBuf};

use app_update::{Idle, Phase, PublicKey, ReleaseIdentity, Sha, state};
use tracing::info;

use crate::fs;
use crate::layout::{APP_EXE, BUNDLE, LAUNCHER_EXE, Layout, Platform, bundle_bin_dir};
use crate::plan::Link;
use crate::refusal::Refusal;

const TARGET: &str = "ducktape::update";
const DESKTOP_TEMPLATE: &str = include_str!("../../../app/packaging/dev.ducktape.app.desktop");

pub fn install(layout: &Layout, from: &Path, release_key: Option<&str>) -> Result<Sha, Refusal> {
    let pin = release_key
        .map(|hex| pinnable_key(layout, hex))
        .transpose()?;
    let identity = release_identity(&identity_path(layout.platform, from))?;
    let sha = match layout.platform {
        Platform::Linux => install_linux(layout, from, identity.as_ref()),
        Platform::MacOs => install_macos(layout, from, identity.as_ref()),
    }?;
    if let Some(key) = pin {
        fs::persist(&layout.release_key_path(), &format!("{key}\n"))?;
    }
    Ok(sha)
}

/// `--release-key`, checked before the install touches anything: 64 hex
/// characters, and the key already pinned if there is one. A different pin
/// (or a file that is not this key) is refused, never replaced — the
/// channel rotates a key through a signed successor, not an install.
fn pinnable_key(layout: &Layout, hex: &str) -> Result<PublicKey, Refusal> {
    let key: PublicKey = hex.parse().map_err(|_| {
        Refusal::new(
            "release_key_invalid",
            "--release-key takes 64 hex characters",
        )
    })?;
    let path = layout.release_key_path();
    fs::refuse_symlink(&path)?;
    match std_fs::read_to_string(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(key),
        Err(error) => Err(Refusal::io("release_key_unreadable", &path, &error)),
        Ok(pinned) if pinned.parse::<PublicKey>().ok() == Some(key) => Ok(key),
        Ok(_) => Err(Refusal::new(
            "release_key_pinned",
            format!("{} pins a different release key", path.display()),
        )),
    }
}

/// `release.json` at the archive root: beside the executables on Linux,
/// beside `Ducktape.app` on macOS.
fn identity_path(platform: Platform, from: &Path) -> PathBuf {
    let root = match platform {
        Platform::Linux => from,
        Platform::MacOs => from.parent().unwrap_or(Path::new("")),
    };
    root.join(ReleaseIdentity::FILE)
}

/// The release's own identity, `None` when it carries none. A file that is
/// there but is not exactly `{sequence, display}` is refused, never read as
/// "unknown".
fn release_identity(path: &Path) -> Result<Option<ReleaseIdentity>, Refusal> {
    fs::refuse_symlink(path)?;
    match std_fs::read_to_string(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Refusal::io("release_identity_unreadable", path, &error)),
        Ok(text) => ReleaseIdentity::decode(&text).map(Some).map_err(|error| {
            Refusal::new(
                "release_identity_invalid",
                format!("{}: {error}", path.display()),
            )
        }),
    }
}

/// The state a clean install leaves: this release, nothing to roll back to,
/// and the release's own sequence (0 for a build that names none).
fn fresh_idle(sha: Sha, identity: Option<&ReleaseIdentity>) -> Phase {
    Phase::Idle(Idle {
        current: sha,
        previous: None,
        pinned_sequence: identity.map_or(0, |identity| identity.sequence),
    })
}

fn install_linux(
    layout: &Layout,
    from: &Path,
    identity: Option<&ReleaseIdentity>,
) -> Result<Sha, Refusal> {
    let source_app = from.join(APP_EXE);
    fs::require_executable(&source_app)
        .map_err(|refusal| Refusal::new("source_app_missing", refusal.detail))?;
    let sha = fs::digest_file(&source_app)?;
    std_fs::create_dir_all(&layout.install_dir)
        .map_err(|error| Refusal::io("install_root_not_writable", &layout.install_dir, &error))?;
    fs::require_writable_install_dir(&layout.install_dir)?;
    seed_linux(layout, from, sha)?;
    fs::replace_symlink(&Link {
        path: layout.current_link(),
        target: Layout::link_target(sha),
    })?;
    remove_leftover(&layout.previous_link())?;
    fs::persist(
        &layout.state_path(),
        &state::encode(&fresh_idle(sha, identity)),
    )?;
    keep_only(&layout.releases_dir(), Some(sha))?;
    write_desktop_entry(layout)?;
    info!(target: TARGET, event = "app_update_installed", release = %sha);
    Ok(sha)
}

/// `releases/<sha>/{ducktape-app, ducktape-launcher}`, built beside its
/// final name and renamed into place.
fn seed_linux(layout: &Layout, from: &Path, sha: Sha) -> Result<(), Refusal> {
    let release_dir = layout.release_dir(sha);
    fs::refuse_symlink(&release_dir)?;
    if release_dir.exists() {
        return fs::require_release(&release_dir, &release_dir);
    }
    let seed = seed_dir(&release_dir);
    let _ = std_fs::remove_dir_all(&seed);
    std_fs::create_dir_all(&seed).map_err(|error| Refusal::io("seed_failed", &seed, &error))?;
    fs::install_file(&from.join(APP_EXE), &seed.join(APP_EXE), 0o755)?;
    fs::install_file(&launcher_source(from)?, &seed.join(LAUNCHER_EXE), 0o755)?;
    std_fs::rename(&seed, &release_dir)
        .map_err(|error| Refusal::io("seed_failed", &release_dir, &error))
}

fn install_macos(
    layout: &Layout,
    from: &Path,
    identity: Option<&ReleaseIdentity>,
) -> Result<Sha, Refusal> {
    let source_bin = bundle_bin_dir(from);
    fs::require_executable(&source_bin.join(APP_EXE))
        .map_err(|refusal| Refusal::new("source_app_missing", refusal.detail))?;
    let sha = fs::digest_file(&source_bin.join(APP_EXE))?;
    let installed = layout.installed_bundle();
    fs::refuse_symlink(&installed)?;
    std_fs::create_dir_all(layout.releases_dir())
        .map_err(|error| Refusal::io("seed_failed", &layout.releases_dir(), &error))?;
    fs::require_writable_install_dir(&layout.install_dir)?;
    seed_macos(layout, from, sha)?;
    // The installed bundle IS the current release on macOS, so the old one is
    // set aside only for the instant the new one takes its name.
    let aside = fs::tmp_name(&installed);
    remove_leftover(&aside)?;
    if installed.exists() {
        fs::move_tree(&installed, &aside)?;
    }
    fs::move_tree(&layout.staged_bundle(sha), &installed)?;
    remove_leftover(&aside)?;
    remove_leftover(&layout.previous_link())?;
    remove_leftover(&layout.journal_path())?;
    fs::persist(
        &layout.state_path(),
        &state::encode(&fresh_idle(sha, identity)),
    )?;
    keep_only(&layout.releases_dir(), None)?;
    info!(target: TARGET, event = "app_update_installed", release = %sha);
    Ok(sha)
}

/// `releases/<sha>/Ducktape.app`, a `ditto` copy with this launcher added
/// when the source bundle carries none.
fn seed_macos(layout: &Layout, from: &Path, sha: Sha) -> Result<(), Refusal> {
    let release_dir = layout.release_dir(sha);
    fs::refuse_symlink(&release_dir)?;
    let staged = layout.staged_bundle(sha);
    if staged.exists() {
        return fs::require_release(&staged, &bundle_bin_dir(&staged));
    }
    let seed = seed_dir(&release_dir);
    let _ = std_fs::remove_dir_all(&seed);
    std_fs::create_dir_all(&seed).map_err(|error| Refusal::io("seed_failed", &seed, &error))?;
    let bundle = seed.join(BUNDLE);
    fs::copy_bundle(from, &bundle)?;
    let launcher = bundle_bin_dir(&bundle).join(LAUNCHER_EXE);
    if fs::require_executable(&launcher).is_err() {
        fs::install_file(&own_exe()?, &launcher, 0o755)?;
    }
    std_fs::rename(&seed, &release_dir)
        .map_err(|error| Refusal::io("seed_failed", &release_dir, &error))
}

/// Remove a file, link or tree an earlier install or update left at `path`.
/// Nothing there is the clean case, not a failure.
fn remove_leftover(path: &Path) -> Result<(), Refusal> {
    let Ok(meta) = std_fs::symlink_metadata(path) else {
        return Ok(());
    };
    let removed = match meta.is_dir() {
        true => std_fs::remove_dir_all(path),
        false => std_fs::remove_file(path),
    };
    removed.map_err(|error| Refusal::io("leftover_not_removed", path, &error))
}

/// Empty `releases/` of everything but `keep`: older releases, a release the
/// update path staged, a seed an interrupted install abandoned.
fn keep_only(releases_dir: &Path, keep: Option<Sha>) -> Result<(), Refusal> {
    let kept = keep.map(|sha| sha.to_string());
    let entries = match std_fs::read_dir(releases_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Refusal::io("leftover_not_removed", releases_dir, &error)),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| Refusal::io("leftover_not_removed", releases_dir, &error))?;
        let is_kept = kept.as_deref() == entry.file_name().to_str();
        if !is_kept {
            remove_leftover(&entry.path())?;
        }
    }
    Ok(())
}

/// The launcher a release ships: the source's own, else this executable.
fn launcher_source(from: &Path) -> Result<PathBuf, Refusal> {
    let shipped = from.join(LAUNCHER_EXE);
    match fs::require_executable(&shipped) {
        Ok(()) => Ok(shipped),
        Err(_) => own_exe(),
    }
}

fn own_exe() -> Result<PathBuf, Refusal> {
    std::env::current_exe().map_err(|error| Refusal::new("self_unknown", error.to_string()))
}

fn seed_dir(release_dir: &Path) -> PathBuf {
    fs::tmp_name(release_dir)
}

/// `~/.local/share/applications/dev.ducktape.app.desktop` with `Exec=` at
/// `<data>/current/ducktape-launcher %u`: the entry keeps its name and
/// `StartupWMClass`, so the window the app opens still associates with it.
fn write_desktop_entry(layout: &Layout) -> Result<(), Refusal> {
    let entry = layout.desktop_entry();
    let exec = exec_argument(&layout.launcher_exec_path());
    let text = DESKTOP_TEMPLATE.replace("@EXEC@", &exec);
    fs::persist(&entry, &text)
}

/// A path as ONE `Exec` argument, per the Desktop Entry spec: quoted, with
/// `"`, `` ` ``, `$` and `\` backslash-escaped inside the quotes and `%`
/// doubled (a field code otherwise); then the file's string escaping, which
/// doubles every backslash again and spells a newline `\n`. Unquoted, a data
/// home with a space splits the path and the session drops the entry.
// ponytail: GLib checks the program exists before it expands `%%`, so a
// data home holding `%` stays hidden on GLib desktops however it is spelled;
// the spec's `%%` is kept for the launchers that follow it.
fn exec_argument(path: &Path) -> String {
    let mut argument = String::from("\"");
    for c in path.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' => {
                argument.push_str("\\\\");
                argument.push(c);
            }
            '\\' => argument.push_str("\\\\\\\\"),
            '%' => argument.push_str("%%"),
            '\n' => argument.push_str("\\n"),
            _ => argument.push(c),
        }
    }
    argument.push('"');
    argument
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_template_carries_the_exec_placeholder_and_the_app_id() {
        assert!(DESKTOP_TEMPLATE.contains("Exec=@EXEC@ %u"));
        assert!(DESKTOP_TEMPLATE.contains("StartupWMClass=dev.ducktape.app"));
        assert!(DESKTOP_TEMPLATE.contains("MimeType=x-scheme-handler/duck;"));
    }

    #[test]
    fn the_exec_argument_is_one_quoted_argument_whatever_the_path_holds() {
        assert_eq!(
            exec_argument(Path::new(
                "/home/op/.local/share/ducktape/current/ducktape-launcher"
            )),
            r#""/home/op/.local/share/ducktape/current/ducktape-launcher""#
        );
        assert_eq!(
            exec_argument(Path::new(r#"/d a/"q"/$v/`c`/b\s/50%/ducktape-launcher"#)),
            r#""/d a/\\"q\\"/\\$v/\\`c\\`/b\\\\s/50%%/ducktape-launcher""#
        );
        assert_eq!(exec_argument(Path::new("/a\nb")), r#""/a\nb""#);
    }
}
