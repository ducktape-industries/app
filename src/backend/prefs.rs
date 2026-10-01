//! `prefs.json` in the config directory. Appearance and motion are read
//! and written here; the recent-nodes list (`endpoints`) and the
//! notification prefs (`runtime::notify`) go through [`read_prefs`] and
//! [`write_prefs`] too.

#[cfg(not(test))]
fn prefs_path() -> Option<std::path::PathBuf> {
    super::config_dir().ok().map(|dir| dir.join("prefs.json"))
}

#[cfg(not(test))]
pub(crate) fn read_prefs() -> serde_json::Value {
    let _timed = crate::perf::time(crate::perf::Key::Shell, "io.read_prefs");
    prefs_path().map_or_else(|| serde_json::json!({}), |path| read_prefs_at(&path))
}

#[cfg(not(test))]
pub(crate) fn write_prefs(prefs: &serde_json::Value) -> bool {
    let _timed = crate::perf::time(crate::perf::Key::Shell, "io.write_prefs");
    prefs_path().is_some_and(|path| write_prefs_at(&path, prefs))
}

/// What `path` holds; empty when it is missing, unreadable or unparsable
/// (the last is set aside as `prefs.json.bad`).
fn read_prefs_at(path: &std::path::Path) -> serde_json::Value {
    super::read_or_set_aside(path, |bytes| serde_json::from_slice(bytes))
        .inspect_err(|error| {
            tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "prefs unreadable, starting empty");
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_prefs_at(path: &std::path::Path, prefs: &serde_json::Value) -> bool {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    serde_json::to_vec_pretty(prefs)
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
pub(crate) fn read_prefs() -> serde_json::Value {
    PREFS.with(|prefs| prefs.borrow().clone())
}

#[cfg(test)]
pub(crate) fn write_prefs(prefs: &serde_json::Value) -> bool {
    PREFS.with(|kept| *kept.borrow_mut() = prefs.clone());
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
    match read_prefs()["appearance"].as_str() {
        Some("light") => Appearance::Light,
        Some("dark") => Appearance::Dark,
        _ => Appearance::System,
    }
}

pub(crate) fn save_appearance(mode: Appearance) -> bool {
    let mut prefs = read_prefs();
    match mode {
        Appearance::System => {
            if let Some(prefs) = prefs.as_object_mut() {
                prefs.remove("appearance");
            }
        }
        Appearance::Light => prefs["appearance"] = serde_json::json!("light"),
        Appearance::Dark => prefs["appearance"] = serde_json::json!("dark"),
    }
    write_prefs(&prefs)
}

/// Whether the launcher's drawings move; on unless turned off.
pub(crate) fn load_motion() -> bool {
    read_prefs()["motion"].as_bool().unwrap_or(true)
}

pub(crate) fn save_motion(on: bool) -> bool {
    let mut prefs = read_prefs();
    prefs["motion"] = serde_json::json!(on);
    write_prefs(&prefs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `prefs.json` cut short is set aside whole as `.bad`, the app
    /// starts with no prefs, and the next write lands whole.
    #[test]
    fn unparsable_prefs_are_set_aside_and_the_next_write_lands_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, br#"{"appearance": "da"#).unwrap();
        assert_eq!(read_prefs_at(&path), serde_json::json!({}));
        assert_eq!(
            std::fs::read(dir.path().join("prefs.json.bad")).unwrap(),
            br#"{"appearance": "da"#
        );
        assert!(!path.exists());
        let prefs = serde_json::json!({"appearance": "dark", "motion": false});
        assert!(write_prefs_at(&path, &prefs));
        assert_eq!(read_prefs_at(&path), prefs);
    }
}
