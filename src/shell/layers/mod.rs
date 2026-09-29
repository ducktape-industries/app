//! The layers one OS window stacks: each a view of its own over the
//! shell's entities, cached where it can be.

mod cached;

pub(crate) use cached::cached_unless_a11y;
