//! The person's word on notices: this device's settings in the prefs file
//! (banners at all, banners in front, the burst limit) and their
//! per-view [`Permission`], read off disk once and kept, and saved by the
//! settings and permission-bar screens.

#[cfg(any(test, feature = "test-support"))]
use std::cell::RefCell;
use std::collections::BTreeMap;

use super::center::Center;
use crate::backend::{edit_prefs, read_prefs};

/// The prefs keys, device-global like `appearance`.
pub(super) const NOTIFY_PREF: &str = "desktop_notifications";
pub(super) const FRONT_PREF: &str = "notify_in_front";
pub(super) const BURST_PREF: &str = "notify_burst";
pub(super) const VIEWS_PREF: &str = "notify_views";

/// The burst limits a person picks from, banners a minute per view.
pub const BURSTS: [u32; 3] = [3, 6, 12];
const DEFAULT_BURST: u32 = 6;

/// The person's word on one view's notices; no word yet is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Allow,
    Silent,
    Block,
}

impl Permission {
    pub const ALL: [Self; 3] = [Self::Allow, Self::Silent, Self::Block];

    pub fn word(self) -> &'static str {
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
pub struct Settings {
    /// Banners at all. Default on: only an explicit `false` turns it off.
    pub banners: bool,
    /// Banners for the window in front, too. Default off.
    pub in_front: bool,
    /// Banners a minute, per view.
    pub burst: u32,
    pub views: BTreeMap<String, Permission>,
    /// The word on a program `views` holds none for: `None` logs its
    /// notices, rings its calls and asks the person (the permission bar); a
    /// host with no one to ask answers for them (`crates/pushd`), and so
    /// does a device whose OS asks for the whole app
    /// (`OsFacts::os_asks_notices`): Allow.
    pub unasked: Option<Permission>,
}

/// The settings as last read: forgotten when a notice setting is saved,
/// and replaced by every [`Settings::load`]. One for the process, as the
/// prefs file is: views post on the window thread, notifiers on their own
/// (`runtime::notifier`), and the shell saves on the window thread.
#[cfg(not(any(test, feature = "test-support")))]
static KEPT: std::sync::Mutex<Option<Settings>> = std::sync::Mutex::new(None);

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// A test's prefs are its thread's own (`backend::prefs`), and so are
    /// the settings read off them.
    static KEPT: RefCell<Option<Settings>> = const { RefCell::new(None) };
}

/// `change` on the kept settings.
fn kept<R>(change: impl FnOnce(&mut Option<Settings>) -> R) -> R {
    #[cfg(not(any(test, feature = "test-support")))]
    return change(&mut KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    #[cfg(any(test, feature = "test-support"))]
    KEPT.with(|kept| change(&mut kept.borrow_mut()))
}

impl Settings {
    /// Off the prefs file, and kept for [`Settings::kept`].
    pub fn load() -> Self {
        let asks = crate::contract::os().facts().os_asks_notices;
        let settings = Self::of(&read_prefs().unwrap_or_default(), asks);
        kept(|kept| *kept = Some(settings.clone()));
        settings
    }

    /// As last read, or off the prefs file the first time: what each post
    /// is judged by, without a prefs read per post.
    pub fn kept() -> Self {
        kept(|kept| kept.clone()).unwrap_or_else(Self::load)
    }

    /// `prefs` as settings; `os_asks`: the OS asked for the whole app.
    pub(super) fn of(prefs: &serde_json::Value, os_asks: bool) -> Self {
        let burst = prefs[BURST_PREF]
            .as_u64()
            .and_then(|burst| BURSTS.into_iter().find(|it| u64::from(*it) == burst))
            .unwrap_or(DEFAULT_BURST);
        let views = prefs[VIEWS_PREF]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(program, word)| Some((program.clone(), Permission::of(word.as_str()?)?)))
            .collect();
        Self {
            banners: prefs[NOTIFY_PREF].as_bool().unwrap_or(true),
            in_front: prefs[FRONT_PREF].as_str() == Some("show"),
            burst,
            views,
            unasked: os_asks.then_some(Permission::Allow),
        }
    }
}

/// A notice setting saved: the kept settings are read again on next use.
fn edit(change: impl FnOnce(&mut serde_json::Value)) {
    edit_prefs(change);
    kept(|kept| *kept = None);
}

pub fn save_banners(on: bool) {
    edit(|prefs| prefs[NOTIFY_PREF] = serde_json::json!(on));
}

pub fn save_in_front(show: bool) {
    edit(|prefs| prefs[FRONT_PREF] = serde_json::json!(if show { "show" } else { "hide" }));
}

pub fn save_burst(burst: u32) {
    edit(|prefs| prefs[BURST_PREF] = serde_json::json!(burst));
}

/// The person answered a view: its bar goes, and the word is kept.
/// A program blocked has no notifier running, and one let through again
/// has its own started (`runtime::notifier`).
pub fn set_permission(center: &mut Center, program: &str, permission: Permission) {
    edit(|prefs| {
        if !prefs[VIEWS_PREF].is_object() {
            prefs[VIEWS_PREF] = serde_json::json!({});
        }
        prefs[VIEWS_PREF][program] = serde_json::json!(permission.word());
    });
    center.asking.remove(program);
    crate::runtime::notifier::permission_moved(program, permission);
}

/// "Not now": the bar goes for this run; the view's notices stay logged
/// silently and its calls still ring, and the bar asks again next launch.
pub fn not_now(center: &mut Center, program: &str) {
    center.asking.remove(program);
    center.not_now.insert(program.to_owned());
}
