//! This device's preferences as the shell draws them. Its methods are its
//! one writer: each saves to `prefs.json` (`backend::prefs`,
//! `runtime::notify::settings`) and then edits the compared value, so a
//! choice already made notifies nobody.
use super::Slice;
use crate::backend::{self, Appearance};
use crate::runtime::notify;
use gpui_kit::Context;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Prefs {
    pub(crate) appearance: Appearance,
    /// The OS says dark; counts under `Appearance::System`.
    pub(crate) system_dark: bool,
    pub(crate) motion: bool,
    /// Read off disk once, and again only after a notice setting was saved.
    pub(crate) notify: notify::Settings,
}

impl Prefs {
    /// As the prefs file has them; the theme's own word comes with
    /// `Windows::sync_appearance`.
    pub(crate) fn load() -> Self {
        Self {
            appearance: backend::load_appearance(),
            system_dark: false,
            motion: backend::load_motion(),
            notify: notify::Settings::load(),
        }
    }

    pub(crate) fn dark(&self) -> bool {
        match self.appearance {
            Appearance::Light => false,
            Appearance::Dark => true,
            Appearance::System => self.system_dark,
        }
    }
}

/// Timed as `reducer.desk`, the name qa's hang rule reads (docs/perf.md).
fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.desk")
}

impl Slice<Prefs> {
    pub(crate) fn set_appearance(&mut self, mode: Appearance, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::save_appearance(mode);
        self.edit(|prefs| prefs.appearance = mode, cx);
    }

    pub(crate) fn set_motion(&mut self, on: bool, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::save_motion(on);
        self.edit(|prefs| prefs.motion = on, cx);
    }

    /// What the theme says the OS is: written by whoever syncs the theme.
    pub(crate) fn set_system_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        self.edit(|prefs| prefs.system_dark = dark, cx);
    }

    pub(crate) fn set_notify_banners(&mut self, on: bool, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::save_banners(on);
        self.reload_notify(cx);
    }

    pub(crate) fn set_notify_in_front(&mut self, show: bool, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::save_in_front(show);
        self.reload_notify(cx);
    }

    pub(crate) fn set_notify_burst(&mut self, burst: u32, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::save_burst(burst);
        self.reload_notify(cx);
    }

    /// The notice settings read off disk again: after a view's permission
    /// was saved (`Notifications::permission` writes the file).
    pub(crate) fn reload_notify(&mut self, cx: &mut Context<Self>) {
        let notify = notify::Settings::load();
        self.edit(|prefs| prefs.notify = notify, cx);
    }

    /// Everything read off disk again: the door's walk put the prefs file
    /// back (`layers::Kept::restore`).
    #[cfg(any(test, feature = "ax-door"))]
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        let loaded = Prefs::load();
        self.edit(
            |prefs| {
                prefs.appearance = loaded.appearance;
                prefs.motion = loaded.motion;
                prefs.notify = loaded.notify;
            },
            cx,
        );
    }
}
