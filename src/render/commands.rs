//! What a guest asks of the mounted tree between frames: widget commands
//! (`host.widget`: focus, cursor, scroll, editor action) resolved against
//! the paths this tree mounts, the walk that names those paths, the
//! user-activation mark an event may spend, and the entry a dialog gives
//! the keyboard when it opens.

use super::*;

/// What a scroll widget command asks of a scrolling container's offset.
#[derive(Clone, Copy)]
pub(super) enum ScrollRequest {
    Relative(f32, f32),
    Absolute(f32, f32),
    By(f32, f32),
    End,
}

/// Visits every node with its authored path, depth first.
pub(super) fn walk_authored_paths(
    node: &wire::Node,
    path: &mut AuthoredPath,
    visit: &mut impl FnMut(&wire::Node, &AuthoredPath),
) {
    let entered_scope = crate::render::enter_scope(node, path);
    // Anonymous primitives (notably List) still own retained host state at
    // their current authored ancestry; they do not add a fabricated segment.
    visit(node, path);
    for child in node.children() {
        walk_authored_paths(child, path, visit);
    }
    if entered_scope {
        path.pop();
    }
}

/// Where focus enters a dialog: a node-less element drawn first in it,
/// tracking `entry`. The frame the dialog opens, focus that is still where
/// it was at the end of that frame moves to the first Tab stop after this
/// one — the dialog's first control — so a keyboard is in the dialog it
/// opened, not behind it. Focus the dialog's own content took is left alone.
pub(crate) fn dialog_entry(
    entry: &FocusHandle,
    opened: bool,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    if opened {
        let before = window.focused(cx);
        let entry = entry.clone();
        window.defer(cx, move |window, cx| {
            if window.focused(cx) == before {
                window.focus(&entry, cx);
                window.focus_next(cx);
            }
        });
    }
    div().id("dialog-entry").track_focus(entry)
}

impl ViewTree {
    /// `Some` when `event` came from a real gesture on this tree — a click,
    /// press or select whose handler is the one the last gesture recorded —
    /// spending the mark, so an activation is granted once.
    pub(crate) fn take_user_activation(&self, event: &wire::Event) -> Option<()> {
        let message = match event {
            wire::Event::Message(message)
            | wire::Event::Click {
                handler: message, ..
            }
            | wire::Event::AuxClick {
                handler: message, ..
            }
            | wire::Event::Select {
                handler: message, ..
            } => message,
            _ => return None,
        };
        let actual_click = self.user_activation.get() == Some(*message);
        if !actual_click {
            return None;
        }
        self.user_activation.take().map(|_| ())
    }

    pub fn execute_widget_command(
        &mut self,
        mut command: wire::WidgetCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Vec<u8>, String> {
        use wire::WidgetCommand as C;
        command.validate()?;
        // every command answers unit; the helpers say what went wrong
        match command {
            C::FocusHandle { handle } => {
                let focus = self
                    .guest_focus_targets
                    .get(&handle)
                    .ok_or_else(|| "focus handle is not mounted".to_string())?
                    .clone();
                focus.focus(window, cx);
            }
            C::FocusPrevious => self.focus_relative(false, window, cx)?,
            C::FocusNext => self.focus_relative(true, window, cx)?,
            C::EditorAction { ref target, .. }
            | C::Focus { ref target }
            | C::CursorFront { ref target }
            | C::CursorEnd { ref target }
            | C::Cursor { ref target, .. }
            | C::SelectAll { ref target }
            | C::Select { ref target, .. } => self.input_command(target, &command, window, cx)?,
            C::Snap { target, x, y } => {
                self.scroll_command(&target, ScrollRequest::Relative(x, y), cx)
            }
            C::SnapEnd { target } => self.scroll_command(&target, ScrollRequest::End, cx),
            C::ScrollTo { target, x, y } => {
                self.scroll_command(&target, ScrollRequest::Absolute(x, y), cx)
            }
            C::ScrollBy { target, x, y } => {
                self.scroll_command(&target, ScrollRequest::By(x, y), cx)
            }
        }
        Ok(wire::encode(&()))
    }

    /// The full authored path of the node a widget command's target names.
    /// A guest sends a suffix (the node's own id, maybe a parent or two),
    /// never the ancestors other code owns, while every retained map is
    /// keyed by the full walked ancestry: any path ending with the target
    /// matches, and the first in depth-first order wins.
    pub(super) fn resolve_target(&self, target: &[wire::ElementIdWire]) -> Option<AuthoredPath> {
        if target.is_empty() {
            return None;
        }
        let mut found = None;
        walk_authored_paths(&self.root, &mut Vec::new(), &mut |_, path| {
            if found.is_none() && path.ends_with(target) {
                found = Some(path.clone());
            }
        });
        found
    }

    pub(super) fn target_focused(
        &self,
        target: &[wire::ElementIdWire],
        window: &Window,
        cx: &App,
    ) -> bool {
        let Some(target) = self.resolve_target(target) else {
            return false;
        };
        let target = target.as_slice();
        if let Some((_, handle)) = self.focus_targets.get(target) {
            return handle.is_focused(window);
        }
        if let Some(field) = self.fields.get(target) {
            return field.state.read(cx).focus_handle(cx).is_focused(window);
        }
        self.editors
            .get(target)
            .is_some_and(|editor| editor.view.is_focused(window, cx))
    }

    pub(super) fn focus_relative(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mut targets = Vec::new();
        walk_authored_paths(&self.root, &mut Vec::new(), &mut |_, path| {
            let available = self.mounted.contains(path)
                && (self.focus_targets.contains_key(path) || self.editors.contains_key(path));
            if available {
                targets.push(path.clone());
            }
        });
        if targets.is_empty() {
            return Ok(());
        }
        let current = targets
            .iter()
            .position(|key| self.target_focused(key, window, cx));
        let index = match (current, forward) {
            (Some(index), true) => (index + 1) % targets.len(),
            (Some(index), false) => (index + targets.len() - 1) % targets.len(),
            (None, true) => 0,
            (None, false) => targets.len() - 1,
        };
        let target = &targets[index];
        self.input_command(
            target,
            &wire::WidgetCommand::Focus {
                target: target.clone(),
            },
            window,
            cx,
        )
    }

    pub(super) fn input_command(
        &mut self,
        target: &[wire::ElementIdWire],
        command: &wire::WidgetCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        use wire::WidgetCommand as C;
        let Some(target) = self.resolve_target(target) else {
            return Ok(());
        };
        let target = target.as_slice();
        if matches!(command, C::Focus { .. }) {
            let mut kind = None;
            walk_authored_paths(&self.root, &mut Vec::new(), &mut |node, path| {
                if path == target
                    && matches!(node, wire::Node::Container(view_wire::ContainerNode { .. }))
                {
                    kind = Some(std::mem::discriminant(node));
                }
            });
            if let Some(kind) = kind {
                let (_, handle) = self
                    .focus_targets
                    .entry(target.to_vec())
                    .or_insert_with(|| (kind, cx.focus_handle()));
                handle.focus(window, cx);
                cx.notify();
                return Ok(());
            }
        }
        // A toolbar press names a tag, not an edit: it goes to the guest's
        // binding as an interaction on that field's document. The native
        // editor never decides what a view's tag means.
        if let C::EditorAction { tag, .. } = command {
            let editor_mounted = self.editors.contains_key(target);
            let Some(store) = editor_mounted
                .then_some(self.editor_store.as_ref())
                .flatten()
            else {
                return Err("editor action target is not a mounted editor".into());
            };
            store.act(target, tag.clone());
            return Ok(());
        }
        if let Some(editor) = self.editors.get(target) {
            editor.view.widget_command(command, window, cx);
            return Ok(());
        }
        // a plain field answers only Focus; its cursor and selection
        // commands are taken and dropped
        if let Some(field) = self.fields.get(target) {
            if matches!(command, C::Focus { .. }) {
                field.state.update(cx, |field, cx| field.focus(window, cx));
            }
            return Ok(());
        }
        Ok(())
    }

    pub(super) fn scroll_command(
        &mut self,
        target: &[wire::ElementIdWire],
        request: ScrollRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.resolve_target(target) else {
            return;
        };
        let target = target.as_slice();
        let Some(handle) = self.scrolls.get(target) else {
            return;
        };
        // an offset is measured from the start: gpui scrolls into the negative
        let maximum = handle.max_offset();
        let next = match request {
            ScrollRequest::Relative(x, y) => {
                point(-px(x * f32::from(maximum.x)), -px(y * f32::from(maximum.y)))
            }
            ScrollRequest::Absolute(x, y) => point(-px(x), -px(y)),
            ScrollRequest::By(x, y) => handle.offset() - point(px(x), px(y)),
            ScrollRequest::End => -maximum,
        };
        handle.set_offset(point(
            next.x.clamp(-maximum.x, px(0.0)),
            next.y.clamp(-maximum.y, px(0.0)),
        ));
        cx.notify();
    }
}
