//! This device's preferences as the shell draws them. Bridged from the
//! model until s11's methods write them (and the disk).
use crate::runtime::notify;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Prefs {
    pub(crate) appearance: crate::Appearance,
    /// The OS says dark; counts under `Appearance::System`.
    pub(crate) system_dark: bool,
    pub(crate) motion: bool,
    /// Read off disk once, and again only after a notice setting was saved.
    pub(crate) notify: notify::Settings,
}

impl Prefs {
    pub(crate) fn dark(&self) -> bool {
        match self.appearance {
            crate::Appearance::Light => false,
            crate::Appearance::Dark => true,
            crate::Appearance::System => self.system_dark,
        }
    }
}
