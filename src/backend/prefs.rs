//! `prefs.json` in the config directory. Appearance and motion are read
//! and written here; the recent-nodes list (`endpoints`) and the
//! notification prefs (`runtime::notify`) go through [`read_prefs`] and
//! [`write_prefs`] too.

use std::path::PathBuf;

fn prefs_path() -> Option<PathBuf> {
    super::config_dir().ok().map(|dir| dir.join("prefs.json"))
}

pub(crate) fn read_prefs() -> serde_json::Value {
    let Some(path) = prefs_path() else {
        return serde_json::json!({});
    };
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

pub(crate) fn write_prefs(prefs: &serde_json::Value) -> bool {
    let Some(path) = prefs_path() else {
        return false;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(bytes) = serde_json::to_vec_pretty(prefs) else {
        return false;
    };
    std::fs::write(&path, bytes).is_ok()
}

pub(crate) fn load_appearance() -> crate::Appearance {
    match read_prefs()["appearance"].as_str() {
        Some("light") => crate::Appearance::Light,
        Some("dark") => crate::Appearance::Dark,
        _ => crate::Appearance::System,
    }
}

pub(crate) fn save_appearance(mode: crate::Appearance) -> bool {
    let mut prefs = read_prefs();
    match mode {
        crate::Appearance::System => {
            if let Some(prefs) = prefs.as_object_mut() {
                prefs.remove("appearance");
            }
        }
        crate::Appearance::Light => prefs["appearance"] = serde_json::json!("light"),
        crate::Appearance::Dark => prefs["appearance"] = serde_json::json!("dark"),
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
