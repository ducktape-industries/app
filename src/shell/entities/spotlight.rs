//! ⌘K in one window: the typed text and the picked row. Bridged from the
//! model until s8's methods.

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Spotlight {
    pub(crate) query: String,
    pub(crate) pick: usize,
}
