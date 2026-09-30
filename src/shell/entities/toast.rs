//! The one-line notice up; empty when none. Bridged from the model until
//! s11's `Toast::show` owns its dismissal.
use super::Slice;

pub(crate) type Toast = Slice<String>;
