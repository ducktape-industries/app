//! What is open over one window's desk. Only the console's opens anything;
//! a pop-out's stays shut. Bridged from the model until s8's methods.
use super::Slice;
use crate::{Popover, SettingsPage};

/// One thing open over the desk. Settings carries its page, so a page
/// click moves this, and a keystroke in Spotlight does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    Spotlight,
    Approve,
    Settings(SettingsPage),
    Network,
    Menu(Popover),
}

pub(crate) type Overlays = Slice<Option<Overlay>>;
