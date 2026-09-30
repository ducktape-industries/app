//! The shell's state as entities the layers observe: app-wide ones and
//! one set per window, each written by its own methods.

mod seats;
mod slice;

pub(crate) use seats::Seats;
