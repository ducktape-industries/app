//! `prefs.json` in the config directory. Appearance, motion and the layout
//! are read and written here; the recent-nodes list (`endpoints`) and the
//! notification prefs (`runtime::notify`) go through [`read_prefs`] and
//! [`edit_prefs`] too.

#[cfg(not(test))]
fn prefs_path() -> Option<std::path::PathBuf> {
    super::config_dir().ok().map(|dir| dir.join("prefs.json"))
}

/// What `prefs.json` holds. A missing file reads as `{}`; an unreadable one
/// is an error, so no write lands over keys this read could not see.
#[cfg(not(test))]
pub(crate) fn read_prefs() -> std::io::Result<serde_json::Value> {
    let _timed = crate::perf::time(crate::perf::Key::Shell, "io.read_prefs");
    prefs_path().map_or_else(|| Ok(serde_json::json!({})), |path| read_prefs_at(&path))
}

/// Reads, changes and writes back `prefs.json`; written only where `change`
/// moved something. False when nothing landed, as when the file could not be
/// read.
#[cfg(not(test))]
pub(crate) fn edit_prefs(change: impl FnOnce(&mut serde_json::Value)) -> bool {
    let _timed = crate::perf::time(crate::perf::Key::Shell, "io.write_prefs");
    prefs_path().is_some_and(|path| edit_prefs_at(&path, change))
}

/// What `path` holds; `{}` when it is missing or unparsable (the last is set
/// aside as `prefs.json.bad`). Any other read error stays an error.
fn read_prefs_at(path: &std::path::Path) -> std::io::Result<serde_json::Value> {
    super::read_or_set_aside(path, |bytes| serde_json::from_slice(bytes))
        .map(|prefs| prefs.unwrap_or_else(|| serde_json::json!({})))
        .inspect_err(|error| {
            tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "prefs unreadable");
        })
}

fn edit_prefs_at(path: &std::path::Path, change: impl FnOnce(&mut serde_json::Value)) -> bool {
    let Ok(mut prefs) = read_prefs_at(path) else {
        return false;
    };
    let before = prefs.clone();
    change(&mut prefs);
    if prefs == before {
        return true;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    serde_json::to_vec_pretty(&prefs)
        .ok()
        .is_some_and(|bytes| super::atomic_write(path, &bytes, false).is_ok())
}

#[cfg(test)]
thread_local! {
    /// A test's own prefs: each test's thread starts with none, and no
    /// test reaches this machine's file (the audit's arrows pick Settings'
    /// choices, and a choice is saved).
    static PREFS: std::cell::RefCell<serde_json::Value> =
        std::cell::RefCell::new(serde_json::json!({}));
}

#[cfg(test)]
thread_local! {
    /// How many times this test's thread read its prefs.
    static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn read_prefs() -> std::io::Result<serde_json::Value> {
    READS.with(|reads| reads.set(reads.get() + 1));
    Ok(PREFS.with(|prefs| prefs.borrow().clone()))
}

/// How many times this test's thread has read its prefs.
#[cfg(test)]
pub(crate) fn prefs_reads() -> usize {
    READS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn edit_prefs(change: impl FnOnce(&mut serde_json::Value)) -> bool {
    PREFS.with(|kept| change(&mut kept.borrow_mut()));
    true
}

/// Light, dark, or as the OS says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    System,
    Light,
    Dark,
}

pub(crate) fn load_appearance() -> Appearance {
    match read_prefs().unwrap_or_default()["appearance"].as_str() {
        Some("light") => Appearance::Light,
        Some("dark") => Appearance::Dark,
        _ => Appearance::System,
    }
}

pub(crate) fn save_appearance(mode: Appearance) -> bool {
    edit_prefs(|prefs| match mode {
        Appearance::System => {
            if let Some(prefs) = prefs.as_object_mut() {
                prefs.remove("appearance");
            }
        }
        Appearance::Light => prefs["appearance"] = serde_json::json!("light"),
        Appearance::Dark => prefs["appearance"] = serde_json::json!("dark"),
    })
}

/// Where the programs are listed: across the top, or down the side. Per
/// device, like the theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Layout {
    #[default]
    MenuBar,
    Sidebar,
}

/// The layout this device picked; none until the launcher's last step
/// asked (`Screen::Layout`).
pub(crate) fn load_layout() -> Option<Layout> {
    match read_prefs().unwrap_or_default()["layout"].as_str() {
        Some("menu_bar") => Some(Layout::MenuBar),
        Some("sidebar") => Some(Layout::Sidebar),
        _ => None,
    }
}

pub(crate) fn save_layout(layout: Layout) -> bool {
    let word = match layout {
        Layout::MenuBar => "menu_bar",
        Layout::Sidebar => "sidebar",
    };
    edit_prefs(|prefs| prefs["layout"] = serde_json::json!(word))
}

/// Whether the launcher's drawings move; on unless turned off.
pub(crate) fn load_motion() -> bool {
    read_prefs().unwrap_or_default()["motion"]
        .as_bool()
        .unwrap_or(true)
}

pub(crate) fn save_motion(on: bool) -> bool {
    edit_prefs(|prefs| prefs["motion"] = serde_json::json!(on))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No `layout` key is "not chosen yet"; each choice reads back as
    /// saved, under its own word.
    #[test]
    fn the_layout_is_none_until_chosen_and_reads_back_as_saved() {
        assert_eq!(load_layout(), None);
        assert!(save_layout(Layout::Sidebar));
        assert_eq!(load_layout(), Some(Layout::Sidebar));
        assert_eq!(read_prefs().unwrap()["layout"], "sidebar");
        assert!(save_layout(Layout::MenuBar));
        assert_eq!(load_layout(), Some(Layout::MenuBar));
        assert_eq!(read_prefs().unwrap()["layout"], "menu_bar");
    }

    /// A `prefs.json` cut short is set aside whole as `.bad`, the app
    /// starts with no prefs, and the next write lands whole.
    #[test]
    fn unparsable_prefs_are_set_aside_and_the_next_write_lands_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, br#"{"appearance": "da"#).unwrap();
        assert_eq!(read_prefs_at(&path).unwrap(), serde_json::json!({}));
        assert_eq!(
            std::fs::read(dir.path().join("prefs.json.bad")).unwrap(),
            br#"{"appearance": "da"#
        );
        assert!(!path.exists());
        let prefs = serde_json::json!({"appearance": "dark", "motion": false});
        assert!(edit_prefs_at(&path, |now| *now = prefs.clone()));
        assert_eq!(read_prefs_at(&path).unwrap(), prefs);
    }

    /// A `prefs.json` this device cannot read is not taken for `{}`: a
    /// save is refused and every key in the file stays.
    #[test]
    fn unreadable_prefs_refuse_a_save_and_keep_every_key() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let kept = br#"{"appearance": "dark", "endpoints": []}"#;
        std::fs::write(&path, kept).unwrap();
        if !crate::backend::unreadable_for_test(&path) {
            return;
        }
        assert!(read_prefs_at(&path).is_err());
        let saved = edit_prefs_at(&path, |prefs| prefs["motion"] = serde_json::json!(false));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!saved);
        assert_eq!(std::fs::read(&path).unwrap(), kept);
    }
}
