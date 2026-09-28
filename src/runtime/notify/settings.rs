//! The person's word on notices: this device's settings in the prefs file
//! (banners at all, banners in front, the burst limit) and their
//! per-view [`Permission`], read on every post and saved by the settings
//! and permission-bar screens.

use std::collections::BTreeMap;

use super::center::center;
use crate::backend::{read_prefs, write_prefs};

/// The prefs keys, device-global like `appearance`.
pub(super) const NOTIFY_PREF: &str = "desktop_notifications";
pub(super) const FRONT_PREF: &str = "notify_in_front";
pub(super) const BURST_PREF: &str = "notify_burst";
pub(super) const VIEWS_PREF: &str = "notify_views";

/// The burst limits a person picks from, banners a minute per view.
pub(crate) const BURSTS: [u32; 3] = [3, 6, 12];
const DEFAULT_BURST: u32 = 6;

/// The person's word on one view's notices; no word yet is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    Allow,
    Silent,
    Block,
}

impl Permission {
    pub(crate) const ALL: [Self; 3] = [Self::Allow, Self::Silent, Self::Block];

    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Allow => "Allow",
            Self::Silent => "Silent",
            Self::Block => "Block",
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
    /// Banners a minute, per view.
    pub(crate) burst: u32,
    pub(crate) views: BTreeMap<String, Permission>,
}

impl Settings {
    pub(crate) fn load() -> Self {
        Self::of(&read_prefs())
    }

    pub(super) fn of(prefs: &serde_json::Value) -> Self {
        let burst = prefs[BURST_PREF]
            .as_u64()
            .and_then(|burst| BURSTS.into_iter().find(|it| u64::from(*it) == burst))
            .unwrap_or(DEFAULT_BURST);
        let views = prefs[VIEWS_PREF]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(module, word)| Some((module.clone(), Permission::of(word.as_str()?)?)))
            .collect();
        Self {
            banners: prefs[NOTIFY_PREF].as_bool().unwrap_or(true),
            in_front: prefs[FRONT_PREF].as_str() == Some("show"),
            burst,
            views,
        }
    }
}

fn edit_prefs(change: impl FnOnce(&mut serde_json::Value)) {
    let mut prefs = read_prefs();
    change(&mut prefs);
    write_prefs(&prefs);
}

pub(crate) fn save_banners(on: bool) {
    edit_prefs(|prefs| prefs[NOTIFY_PREF] = serde_json::json!(on));
}

pub(crate) fn save_in_front(show: bool) {
    edit_prefs(|prefs| prefs[FRONT_PREF] = serde_json::json!(if show { "show" } else { "hide" }));
}

pub(crate) fn save_burst(burst: u32) {
    edit_prefs(|prefs| prefs[BURST_PREF] = serde_json::json!(burst));
}

/// The person answered a view: its bar goes, and the word is kept.
pub(crate) fn set_permission(module: &str, permission: Permission) {
    edit_prefs(|prefs| {
        if !prefs[VIEWS_PREF].is_object() {
            prefs[VIEWS_PREF] = serde_json::json!({});
        }
        prefs[VIEWS_PREF][module] = serde_json::json!(permission.word());
    });
    center().asking.remove(module);
}

/// "Not now": the bar goes for this run; the view's notices stay logged
/// silently, and the bar asks again next launch.
pub(crate) fn not_now(module: &str) {
    let mut center = center();
    center.asking.remove(module);
    center.not_now.insert(module.to_owned());
}
