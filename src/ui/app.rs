//! The app's own state (`Ducktape`) and what still moves it
//! (`AppMessage`): the wall clock's beat. Everything else is the shell's
//! entities' (`shell::entities`): the desks, the windows, the toast, the
//! prefs, the notification centre's counts, the node in hand, the sign-in.
//! s12 deletes this with the beats.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    System,
    Light,
    Dark,
}

/// The app's two shared stores, which the entities read: the notification
/// centre the bell draws (which its views post into) and the programs the
/// node lists (which the bar's tabs, the empty window and Search draw and
/// a link is read against). A test swaps in its own.
pub struct Ducktape {
    pub(crate) center: crate::runtime::notify::CenterHandle,
    pub(crate) roster: crate::runtime::Roster,
}

impl std::fmt::Debug for Ducktape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ducktape")
    }
}

#[derive(Debug)]
pub(crate) enum AppMessage {
    /// The wall clock's beat: moves nothing any more (an open menu's ages
    /// count on their own clock, `layers::Chrome`; the toast takes itself
    /// down, `entities::Toast`); s12 deletes it.
    WallTick,
}

impl Ducktape {
    /// The state at launch.
    pub(crate) fn boot() -> Self {
        Ducktape {
            center: crate::runtime::notify::center().clone(),
            roster: crate::runtime::roster().clone(),
        }
    }
}
