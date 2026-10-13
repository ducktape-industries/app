//! `ducktape-app install`: lay down a CLEAN install of the binary that runs
//! it. Seed `releases/<sha>`, put it at the install path, write `state.json`
//! as a fresh `Idle`, and on Linux register the desktop entry (and its icon)
//! that points the session at `current/ducktape-app`. On macOS the binary's
//! own bundle is copied to `<install>/Ducktape.app` whole.
//!
//! An install is not an update. It keeps nothing of what it found: no
//! `previous` to roll back to, no staged release, no swap journal, and the
//! state it writes knows only the release it just placed. Offering, staging,
//! flipping and rolling back are the update path's, which starts from the
//! state this leaves. So an install never refuses for what is already there:
//! running it again is how a machine gets back to a known set.
//!
//! A release installed this way has no archive, so its identity is the
//! sha256 of its executable, and its pin is the sequence it was published at
//! ([`super::own_sequence`]): the channel publishing that same sequence reads
//! as what already runs, and only a newer one is downloaded.

use std::path::Path;

use app_update::{Idle, Phase, Sha};
use tracing::info;

use super::{APP_EXE, BUNDLE, Layout, Refusal, TARGET};

const DESKTOP_TEMPLATE: &str = include_str!("../../packaging/dev.ducktape.app.desktop");
const ICON: &[u8] = include_bytes!("../../assets/icon.svg");

/// `ducktape-app install`, from the command line: the exit status.
pub(crate) fn install_cli() -> i32 {
    let installed = match (Layout::host(), std::env::current_exe()) {
        (Some(layout), Ok(own)) => install(&layout, &own).map(|sha| (layout, sha)),
        _ => Err(Refusal::new(
            "layout_unresolvable",
            "cannot resolve the app directories: set $HOME",
        )),
    };
    match installed {
        Ok((layout, sha)) => {
            let at = match cfg!(target_os = "macos") {
                true => layout.installed_bundle(),
                false => layout.current_link(),
            };
            println!("installed {sha} at {}", at.display());
            0
        }
        Err(refusal) => {
            eprintln!("install refused: {refusal}");
            1
        }
    }
}

/// Install `own` (the running executable) under `layout`.
pub(super) fn install(layout: &Layout, own: &Path) -> Result<Sha, Refusal> {
    let sha = super::digest_file(own)?;
    match cfg!(target_os = "macos") {
        true => install_macos(layout, own, sha)?,
        false => install_linux(layout, own, sha)?,
    }
    let fresh = Phase::Idle(Idle {
        current: sha,
        previous: None,
        pinned_sequence: super::own_sequence(),
    });
    super::persist_phase(layout, &fresh)?;
    info!(target: TARGET, event = "app_update_installed", release = %sha);
    Ok(sha)
}

fn install_linux(layout: &Layout, own: &Path, sha: Sha) -> Result<(), Refusal> {
    std::fs::create_dir_all(&layout.install)
        .map_err(|error| Refusal::io("install_root_not_writable", &layout.install, &error))?;
    super::require_writable(&layout.install)?;
    let release = layout.release_dir(sha);
    super::refuse_symlink(&release)?;
    if !release.exists() {
        // built beside its final name and renamed into place
        let seed = super::tmp_name(&release);
        super::remove_leftover(&seed)?;
        std::fs::create_dir_all(&seed)
            .map_err(|error| Refusal::io("seed_failed", &seed, &error))?;
        let exe = seed.join(APP_EXE);
        std::fs::copy(own, &exe).map_err(|error| Refusal::io("seed_failed", &exe, &error))?;
        set_executable(&exe)?;
        std::fs::rename(&seed, &release)
            .map_err(|error| Refusal::io("seed_failed", &release, &error))?;
    }
    super::require_release(layout, sha)?;
    super::replace_symlink(&layout.current_link(), &Layout::link_target(sha))?;
    super::remove_leftover(&layout.previous_link())?;
    keep_only(&layout.releases_dir(), Some(sha))?;
    let entry = DESKTOP_TEMPLATE.replace("@EXEC@", &exec_argument(&layout.app_exe()));
    super::persist(&layout.desktop_entry(), entry.as_bytes())?;
    super::persist(&layout.icon(), ICON)
}

fn install_macos(layout: &Layout, own: &Path, sha: Sha) -> Result<(), Refusal> {
    // `<bundle>/Contents/MacOS/ducktape-app`
    let bundle = own
        .ancestors()
        .nth(3)
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .ok_or_else(|| {
            Refusal::new(
                "not_in_a_bundle",
                format!("{} is not inside an .app bundle", own.display()),
            )
        })?;
    let installed = layout.installed_bundle();
    super::refuse_symlink(&installed)?;
    std::fs::create_dir_all(&layout.install)
        .map_err(|error| Refusal::io("install_root_not_writable", &layout.install, &error))?;
    super::require_writable(&layout.install)?;
    let staged = layout.staged_bundle(sha);
    if !staged.exists() {
        let release = layout.release_dir(sha);
        let seed = super::tmp_name(&release);
        super::remove_leftover(&seed)?;
        std::fs::create_dir_all(&seed)
            .map_err(|error| Refusal::io("seed_failed", &seed, &error))?;
        super::macos::copy_bundle(bundle, &seed.join(BUNDLE))?;
        super::remove_leftover(&release)?;
        std::fs::rename(&seed, &release)
            .map_err(|error| Refusal::io("seed_failed", &release, &error))?;
    }
    // the installed bundle IS the current release, so the old one is set
    // aside only for the instant the new one takes its name
    let aside = super::tmp_name(&installed);
    super::remove_leftover(&aside)?;
    if installed.exists() {
        super::move_tree(&installed, &aside)?;
    }
    super::move_tree(&staged, &installed)?;
    super::remove_leftover(&aside)?;
    super::remove_leftover(&layout.previous_link())?;
    super::remove_leftover(&layout.journal_path())?;
    keep_only(&layout.releases_dir(), None)
}

fn set_executable(path: &Path) -> Result<(), Refusal> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| Refusal::io("seed_failed", path, &error))
}

/// Empty `releases/` of everything but `keep`: older releases, a release the
/// update path staged, a partial download, a seed an interrupted install
/// abandoned.
fn keep_only(releases_dir: &Path, keep: Option<Sha>) -> Result<(), Refusal> {
    let kept = keep.map(|sha| sha.to_string());
    let entries = match std::fs::read_dir(releases_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Refusal::io("leftover_not_removed", releases_dir, &error)),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| Refusal::io("leftover_not_removed", releases_dir, &error))?;
        if kept.as_deref() != entry.file_name().to_str() {
            super::remove_leftover(&entry.path())?;
        }
    }
    Ok(())
}

/// A path as ONE `Exec` argument, per the Desktop Entry spec: quoted, with
/// `"`, `` ` ``, `$` and `\` backslash-escaped inside the quotes and `%`
/// doubled (a field code otherwise); then the file's string escaping, which
/// doubles every backslash again and spells a newline `\n`. Unquoted, a data
/// home with a space splits the path and the session drops the entry.
// ponytail: GLib checks the program exists before it expands `%%`, so a data
// home holding `%` stays hidden on GLib desktops however it is spelled; the
// spec's `%%` is kept for the launchers that follow it.
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
        assert!(DESKTOP_TEMPLATE.contains("Icon=ducktape"));
    }

    #[test]
    fn the_exec_argument_is_one_quoted_argument_whatever_the_path_holds() {
        assert_eq!(
            exec_argument(Path::new(
                "/home/op/.local/share/ducktape/current/ducktape-app"
            )),
            r#""/home/op/.local/share/ducktape/current/ducktape-app""#
        );
        assert_eq!(
            exec_argument(Path::new(r#"/d a/"q"/$v/`c`/b\s/50%/ducktape-app"#)),
            r#""/d a/\\"q\\"/\\$v/\\`c\\`/b\\\\s/50%%/ducktape-app""#
        );
        assert_eq!(exec_argument(Path::new("/a\nb")), r#""/a\nb""#);
    }
}
