//! The chain as the node last told it: its height, when that last moved,
//! the node's whole answer. Times are wall seconds (`notify::wall()`), so
//! the open node menu ages them from its own clock. Bridged from the model
//! until s10's poll writes it.
use gpui_kit::Context;

pub(crate) struct Chain {
    /// The node's height; -1 until one answered.
    pub(crate) height: i64,
    /// The wall second the height last moved.
    pub(crate) block_seen: i64,
    pub(crate) node: Option<crate::backend::NodeStatus>,
    /// The wall second the node last answered, however long the height
    /// stood: taken without a notify, since it moves on every answered
    /// poll and only an open node menu's own clock reads it.
    pub(crate) heard: i64,
}

impl Chain {
    /// Takes `next`; notifies only when the height, the time it moved or
    /// the node's answer did. `true` when it notified.
    pub(crate) fn set(&mut self, next: Chain, cx: &mut Context<Self>) -> bool {
        let moved = (self.height, self.block_seen, &self.node)
            != (next.height, next.block_seen, &next.node);
        *self = next;
        if moved {
            cx.notify();
        }
        moved
    }
}
