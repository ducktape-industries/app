//! Where one window's status dot sits: the node button's empty well, as
//! the bar last laid it out. Chrome's probe commits it after the frame from
//! s6b; the dot layer draws at it from s7.
use super::Slice;
use gpui_kit::{Bounds, Pixels};

pub(crate) type DotSlot = Slice<Option<Bounds<Pixels>>>;
