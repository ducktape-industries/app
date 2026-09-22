//! Window-local pane ownership and sizing, independent of native view entities.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const MAX_PANES: usize = 3;
pub(crate) const MIN_PANE_WIDTH: f32 = 320.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pane {
    pub(crate) module: &'static str,
    pub(crate) instance: u64,
    pub(crate) width: f32,
}

impl Pane {
    pub(crate) fn new(module: &'static str) -> Self {
        static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
        Self {
            module,
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            width: MIN_PANE_WIDTH,
        }
    }
}

#[derive(Default, Debug)]
pub(crate) struct Layout {
    pub(crate) panes: Vec<Pane>,
    pub(crate) focused: usize,
}

impl Layout {
    /// Rail selection reuses an existing module before replacing a focused view.
    pub(crate) fn select(&mut self, module: &'static str) -> bool {
        if let Some(index) = self.panes.iter().position(|pane| pane.module == module) {
            return self.focus(index);
        }
        let mut pane = Pane::new(module);
        if let Some(current) = self.panes.get_mut(self.focused) {
            pane.width = current.width;
            *current = pane;
        } else {
            self.panes.push(pane);
            self.focused = 0;
        }
        true
    }

    pub(crate) fn split(&mut self, module: &'static str) -> bool {
        if self.panes.len() == MAX_PANES {
            return false;
        }
        let index = if self.panes.is_empty() {
            0
        } else {
            self.focused + 1
        };
        self.panes.insert(index, Pane::new(module));
        self.focused = index;
        true
    }

    pub(crate) fn focus(&mut self, index: usize) -> bool {
        if index >= self.panes.len() || self.focused == index {
            return false;
        }
        self.focused = index;
        true
    }

    pub(crate) fn close(&mut self, index: usize) -> Option<Pane> {
        if index >= self.panes.len() {
            return None;
        }
        let pane = self.panes.remove(index);
        if index < self.focused {
            self.focused -= 1;
        }
        self.focused = self.focused.min(self.panes.len().saturating_sub(1));
        Some(pane)
    }

    pub(crate) fn popout(&mut self, index: usize) -> Option<Pane> {
        self.close(index)
    }

    /// Returns the displaced view so its entity can be released by the caller.
    pub(crate) fn popin(&mut self, mut pane: Pane) -> Option<Pane> {
        if self.panes.len() == MAX_PANES {
            pane.width = self.panes[self.focused].width;
            Some(std::mem::replace(&mut self.panes[self.focused], pane))
        } else {
            self.panes.push(pane);
            self.focused = self.panes.len() - 1;
            None
        }
    }

    pub(crate) fn resize_pair(&mut self, index: usize, left: f32, right: f32) -> bool {
        if !left.is_finite() || !right.is_finite() {
            return false;
        }
        let Some(pair) = self
            .panes
            .get_mut(index..)
            .and_then(|panes| panes.get_mut(..2))
        else {
            return false;
        };
        pair[0].width = left.max(MIN_PANE_WIDTH);
        pair[1].width = right.max(MIN_PANE_WIDTH);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_selection_focuses_existing_or_replaces_only_focused() {
        let mut layout = Layout::default();
        assert!(layout.select("chat"));
        let chat = layout.panes[0].instance;
        assert!(layout.split("files"));
        assert!(layout.select("chat"));
        assert_eq!(layout.focused, 0);
        assert_eq!(layout.panes[0].instance, chat);
        assert!(!layout.select("chat"));
        layout.panes[0].width = 480.0;
        assert!(layout.select("calendar"));
        assert_eq!(layout.panes.len(), 2);
        assert_eq!(layout.panes[0].module, "calendar");
        assert_eq!(layout.panes[0].width, 480.0);
        assert_ne!(layout.panes[0].instance, chat);
        assert_eq!(layout.panes[1].module, "files");
    }

    #[test]
    fn split_inserts_right_with_unique_instances_and_limits_count() {
        let mut layout = Layout::default();
        assert!(layout.split("chat"));
        assert!(layout.split("files"));
        assert!(layout.focus(0));
        assert!(layout.split("chat"));
        assert_eq!(layout.focused, 1);
        assert_eq!(layout.panes[1].module, "chat");
        assert_eq!(layout.panes[2].module, "files");
        assert_ne!(layout.panes[0].instance, layout.panes[1].instance);
        assert!(!layout.split("calendar"));
        assert_eq!(layout.panes.len(), MAX_PANES);
        assert!(!layout.focus(3));
        assert_eq!(layout.focused, 1);
    }

    #[test]
    fn closing_preserves_focus_identity_and_last_close_is_empty() {
        let mut layout = Layout::default();
        for module in ["chat", "files", "calendar"] {
            layout.split(module);
        }
        let focused = layout.panes[2].instance;
        assert!(layout.close(0).is_some());
        assert_eq!(layout.focused, 1);
        assert_eq!(layout.panes[layout.focused].instance, focused);
        assert!(layout.close(1).is_some());
        assert_eq!(layout.focused, 0);
        assert!(layout.close(0).is_some());
        assert!(layout.panes.is_empty());
        assert_eq!(layout.focused, 0);
        assert!(layout.close(0).is_none());
        assert!(!layout.focus(0));
    }

    #[test]
    fn popout_and_popin_transfer_instance_and_replace_at_capacity() {
        let mut source = Layout::default();
        source.split("chat");
        source.panes[0].width = 450.0;
        let original = source.panes[0].clone();
        let pane = source.popout(0).unwrap();
        assert!(source.panes.is_empty());
        let mut destination = Layout::default();
        assert!(destination.popin(pane).is_none());
        assert_eq!(destination.panes[0], original);
        destination.split("files");
        destination.split("calendar");
        destination.focus(1);
        let displaced = destination.panes[1].instance;
        let pane = Pane::new("chat");
        let incoming = pane.instance;
        assert_eq!(destination.popin(pane).unwrap().instance, displaced);
        assert_eq!(destination.panes.len(), MAX_PANES);
        assert_eq!(destination.focused, 1);
        assert_eq!(destination.panes[1].instance, incoming);
        assert!(destination.popout(3).is_none());
    }

    #[test]
    fn resizing_preserves_other_panes_and_rejects_invalid_input() {
        let mut layout = Layout::default();
        for _ in 0..MAX_PANES {
            layout.split("chat");
        }
        assert!(layout.resize_pair(0, 420.0, 380.0));
        assert_eq!(layout.panes[0].width, 420.0);
        assert_eq!(layout.panes[1].width, 380.0);
        assert_eq!(layout.panes[2].width, MIN_PANE_WIDTH);
        assert!(layout.resize_pair(1, 20.0, 450.0));
        assert_eq!(layout.panes[1].width, MIN_PANE_WIDTH);
        assert_eq!(layout.panes[2].width, 450.0);
        let before = layout.panes.clone();
        assert!(!layout.resize_pair(2, 400.0, 400.0));
        assert!(!layout.resize_pair(usize::MAX, 400.0, 400.0));
        assert!(!layout.resize_pair(0, f32::NAN, 400.0));
        assert!(!layout.resize_pair(0, 400.0, f32::INFINITY));
        assert_eq!(layout.panes, before);
    }
}
