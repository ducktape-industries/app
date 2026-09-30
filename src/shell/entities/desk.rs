//! One window's panes as the model has them: which program is where, their
//! frames on the desk, their stacking, which is in front, which the
//! keyboard holds (`ui::layout::Layout`). Its layers observe it; the bridge
//! in `Desktop::dispatch` is its one writer until s11 gives it methods.
use super::Slice;
use crate::ui::layout::Layout;

pub(crate) type Desk = Slice<Layout>;
