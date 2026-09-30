//! The shell's state as entities the layers observe: app-wide ones and
//! one set per window, each written by its own methods.

mod desk;
mod seats;
mod slice;

pub(crate) use desk::Desk;
pub(crate) use seats::Seats;
pub(crate) use slice::{Observed, Slice};
