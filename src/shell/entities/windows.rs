//! The app's windows. For now only the program in front: the one the bar
//! highlights and an untouched console desk opens (`active`). Bridged from
//! the model; s11 gives it the window list and its methods.
use super::Slice;

pub(crate) type Windows = Slice<Option<&'static str>>;
