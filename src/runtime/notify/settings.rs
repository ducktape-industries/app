//! The person's word on banners: this device's settings in the prefs file
//! (banners at all, banners in front, the burst limit) and their
//! per-program [`Permission`], read off disk once and kept, and saved by
//! the settings page and the question at the top of the centre. What
//! enters at all is the inbox's (its mutes, `Center::set_muted`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::backend::{edit_prefs, read_prefs};

/// The prefs keys, device-global like `appearance`.
pub(super) const NOTIFY_PREF: &str = "desktop_notifications";
pub(super) const FRONT_PREF: &str = "notify_in_front";
pub(super) const BURST_PREF: &str = "notify_burst";
pub(super) const PROGRAMS_PREF: &str = "notify_programs";

/// The burst limits a person picks from, banners a minute per program.
pub(crate) const BURSTS: [u32; 3] = [3, 6, 12];
const DEFAULT_BURST: u32 = 6;

/// The person's word on one program's banners on this device; no word
/// yet is `None`. Keeping a program out altogether is the inbox's mute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    Allow,
    Silent,
}

impl Permission {
    pub(crate) const ALL: [Self; 2] = [Self::Allow, Self::Silent];

    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Allow => "Allow",
            Self::Silent => "Silent",
        }
    }

    fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.word() == word)
    }
}

/// This device's notification settings, as the prefs file holds them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    /// Banners at all. Default on: only an explicit `false` turns it off.
    pub(crate) banners: bool,
    /// Banners for the window in front, too. Default off.
    pub(crate) in_front: bool,
    /// Banners a minute, per program.
    pub(crate) burst: u32,
    pub(crate) programs: BTreeMap<String, Permission>,
}

/// The settings as last read, process-wide: the window thread saves them
/// and the inbox runner judges entries by them on the kernel's thread.
/// A save stores what it wrote, under this lock, so no read in flight
/// can put the pre-save settings back over it.
static KEPT: Mutex<Option<Settings>> = Mutex::new(None);

fn kept_lock() -> std::sync::MutexGuard<'static, Option<Settings>> {
    KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Settings {
    /// Off the prefs file, and kept for [`Settings::kept`].
    pub(crate) fn load() -> Self {
        let mut kept = kept_lock();
        let settings = Self::of(&read_prefs().unwrap_or_default());
        *kept = Some(settings.clone());
        settings
    }

    /// As last read, or off the prefs file the first time: what each
    /// arriving entry is judged by, without a prefs read per entry.
    pub(crate) fn kept() -> Self {
        let kept = kept_lock().clone();
        kept.unwrap_or_else(Self::load)
    }

    /// What the settings hold right now, without a read: `None` before
    /// the first load (a test).
    #[cfg(test)]
    pub(crate) fn kept_now() -> Option<Self> {
        kept_lock().clone()
    }

    pub(super) fn of(prefs: &serde_json::Value) -> Self {
        let burst = prefs[BURST_PREF]
            .as_u64()
            .and_then(|burst| BURSTS.into_iter().find(|it| u64::from(*it) == burst))
            .unwrap_or(DEFAULT_BURST);
        let programs = prefs[PROGRAMS_PREF]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(program, word)| Some((program.clone(), Permission::of(word.as_str()?)?)))
            .collect();
        Self {
            banners: prefs[NOTIFY_PREF].as_bool().unwrap_or(true),
            in_front: prefs[FRONT_PREF].as_str() == Some("show"),
            burst,
            programs,
        }
    }
}

/// A notice setting saved: the kept settings are what was just written.
fn edit(change: impl FnOnce(&mut serde_json::Value)) {
    let mut kept = kept_lock();
    edit_prefs(change);
    *kept = Some(Settings::of(&read_prefs().unwrap_or_default()));
}

pub(crate) fn save_banners(on: bool) {
    edit(|prefs| prefs[NOTIFY_PREF] = serde_json::json!(on));
}

pub(crate) fn save_in_front(show: bool) {
    edit(|prefs| prefs[FRONT_PREF] = serde_json::json!(if show { "show" } else { "hide" }));
}

pub(crate) fn save_burst(burst: u32) {
    edit(|prefs| prefs[BURST_PREF] = serde_json::json!(burst));
}

/// The person answered for a program: the word is kept on this device.
/// Its question in the centre is the caller's to take down, after this
/// (the prefs file is never written under the centre's lock).
pub(crate) fn set_permission(program: &str, permission: Permission) {
    edit(|prefs| {
        if !prefs[PROGRAMS_PREF].is_object() {
            prefs[PROGRAMS_PREF] = serde_json::json!({});
        }
        prefs[PROGRAMS_PREF][program] = serde_json::json!(permission.word());
    });
}
