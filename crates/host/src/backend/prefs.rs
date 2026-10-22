//! `prefs.json` in the config directory. Appearance, motion, the layout and
//! the memory a view and its pictures may hold are read and written here;
//! the recent-nodes list (`endpoints`) and the notification prefs
//! (`runtime::notify`) go through [`read_prefs`] and [`edit_prefs`] too.

fn prefs_path() -> Option<std::path::PathBuf> {
    Some(super::dirs().config.join("prefs.json"))
}

/// What `prefs.json` holds. A missing file reads as `{}`; an unreadable one
/// is an error, so no write lands over keys this read could not see.
pub fn read_prefs() -> std::io::Result<serde_json::Value> {
    let _timed = crate::perf::time(crate::perf::Key::Shell, "io.read_prefs");
    #[cfg(any(test, feature = "test-support"))]
    READS.with(|reads| reads.set(reads.get() + 1));
    prefs_path().map_or_else(|| Ok(serde_json::json!({})), |path| read_prefs_at(&path))
}

/// Reads, changes and writes back `prefs.json`; written only where `change`
/// moved something. False when nothing landed, as when the file could not be
/// read.
pub fn edit_prefs(change: impl FnOnce(&mut serde_json::Value)) -> bool {
    let _timed = crate::perf::time(crate::perf::Key::Shell, "io.write_prefs");
    prefs_path().is_some_and(|path| edit_prefs_at(&path, change))
}

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
    serde_json::to_vec_pretty(&prefs).ok().is_some_and(|bytes| {
        crate::contract::os()
            .disk()
            .write(path, &bytes, false)
            .is_ok()
    })
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times this test's thread has read its prefs.
#[cfg(any(test, feature = "test-support"))]
pub fn prefs_reads() -> usize {
    READS.with(std::cell::Cell::get)
}

/// Light, dark, or as the OS says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

pub fn load_appearance() -> Appearance {
    match read_prefs().unwrap_or_default()["appearance"].as_str() {
        Some("light") => Appearance::Light,
        Some("dark") => Appearance::Dark,
        _ => Appearance::System,
    }
}

pub fn save_appearance(mode: Appearance) -> bool {
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
pub enum Layout {
    #[default]
    MenuBar,
    Sidebar,
}

/// The layout this device picked; none until the launcher's last step
/// asked (`Screen::Layout`).
pub fn load_layout() -> Option<Layout> {
    match read_prefs().unwrap_or_default()["layout"].as_str() {
        Some("menu_bar") => Some(Layout::MenuBar),
        Some("sidebar") => Some(Layout::Sidebar),
        _ => None,
    }
}

pub fn save_layout(layout: Layout) -> bool {
    let word = match layout {
        Layout::MenuBar => "menu_bar",
        Layout::Sidebar => "sidebar",
    };
    edit_prefs(|prefs| prefs["layout"] = serde_json::json!(word))
}

/// Whether the launcher's drawings move; on unless turned off.
pub fn load_motion() -> bool {
    read_prefs().unwrap_or_default()["motion"]
        .as_bool()
        .unwrap_or(true)
}

pub fn save_motion(on: bool) -> bool {
    edit_prefs(|prefs| prefs["motion"] = serde_json::json!(on))
}

/// Whether debug mode is on (the phone's App settings: the perf counters
/// on, what each program costs shown); off unless turned on.
pub fn load_debug() -> bool {
    read_prefs().unwrap_or_default()["debug"]
        .as_bool()
        .unwrap_or(false)
}

pub fn save_debug(on: bool) -> bool {
    edit_prefs(|prefs| prefs["debug"] = serde_json::json!(on))
}

pub fn load_text_size() -> Option<u64> {
    read_prefs().unwrap_or_default()["text_size"].as_u64()
}

pub fn save_text_size(points: Option<u64>) -> bool {
    edit_prefs(|prefs| match points {
        None => {
            if let Some(prefs) = prefs.as_object_mut() {
                prefs.remove("text_size");
            }
        }
        Some(points) => prefs["text_size"] = serde_json::json!(points),
    })
}

/// This device's public key on the network `keyring` names, hex, as last
/// seated there (`Account::unlocked`): whose notices that network's
/// notifiers run for while another is in hand (`runtime::follow`), no key
/// store asked. None before its first seat with this build.
pub fn device_key_of(keyring: &str) -> Option<String> {
    let prefs = read_prefs().ok()?;
    prefs["device_keys"][keyring].as_str().map(str::to_owned)
}

/// `key` (hex) kept as this device's public key on `keyring`'s network:
/// [`device_key_of`] from then on.
pub fn note_device_key(keyring: &str, key: &str) -> bool {
    edit_prefs(|prefs| prefs["device_keys"][keyring] = serde_json::json!(key))
}

const DEFAULT_VIEW_BYTES: u64 = 64 << 20;
const DEFAULT_PICTURE_BYTES: u64 = 64 << 20;
const DEFAULT_SVG_RASTER_BYTES: u64 = 256 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBounds {
    pub view_bytes: u64,
    pub picture_bytes: u64,
    pub svg_raster_bytes: u64,
}

pub fn load_memory_bounds() -> MemoryBounds {
    let prefs = read_prefs().unwrap_or_default();
    let memory = &prefs["memory"];
    let bytes = |key: &str, default: u64| memory[key].as_u64().unwrap_or(default);
    MemoryBounds {
        view_bytes: bytes("view_bytes", DEFAULT_VIEW_BYTES),
        picture_bytes: bytes("picture_bytes", DEFAULT_PICTURE_BYTES),
        svg_raster_bytes: bytes("svg_raster_bytes", DEFAULT_SVG_RASTER_BYTES),
    }
}

pub fn memory_bounds() -> MemoryBounds {
    static BOUNDS: std::sync::OnceLock<MemoryBounds> = std::sync::OnceLock::new();
    *BOUNDS.get_or_init(load_memory_bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_bounds_read_each_key_and_keep_the_default_for_a_missing_one() {
        let defaults = load_memory_bounds();
        assert!(edit_prefs(|prefs| {
            prefs["memory"] = serde_json::json!({"picture_bytes": 1024, "view_bytes": 2048})
        }));
        assert_eq!(
            load_memory_bounds(),
            MemoryBounds {
                view_bytes: 2048,
                picture_bytes: 1024,
                ..defaults
            }
        );
    }

    #[test]
    fn the_appearance_reads_back_as_saved_and_system_is_no_key() {
        assert_eq!(load_appearance(), Appearance::System);
        for mode in [Appearance::Dark, Appearance::Light] {
            assert!(save_appearance(mode));
            assert_eq!(load_appearance(), mode);
        }
        assert!(save_appearance(Appearance::System));
        assert_eq!(load_appearance(), Appearance::System);
        assert!(read_prefs().unwrap().get("appearance").is_none());
    }

    #[test]
    fn debug_mode_is_off_until_turned_on_and_reads_back_as_saved() {
        assert!(!load_debug());
        assert!(save_debug(true));
        assert!(load_debug());
        assert!(save_debug(false));
        assert!(!load_debug());
    }

    #[test]
    fn the_text_size_reads_back_as_saved_and_the_default_is_no_key() {
        assert_eq!(load_text_size(), None);
        assert!(save_text_size(Some(21)));
        assert_eq!(load_text_size(), Some(21));
        assert!(save_text_size(None));
        assert_eq!(load_text_size(), None);
        assert!(read_prefs().unwrap().get("text_size").is_none());
    }

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
