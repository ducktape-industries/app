//! What a guest asks of the mounted tree between frames: widget commands
//! (`host.widget`: focus, cursor, scroll, editor action) on the node at
//! the whole authored path they name, the walk that names those paths, the
//! user-activation mark an event may spend, the entry a dialog gives the
//! keyboard when it opens, and the way back when it closes.

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

/// Where focus enters a dialog that opens this frame: `entry`, the handle
/// its focus trap tracks. Focus that is not in the dialog at the end of the
/// frame moves to the dialog's first Tab stop, as Tab from the trap would
/// take it ([`tab`]) — so a keyboard is in the dialog it opened, not behind
/// it. Focus the dialog's own content took is left alone.
///
/// Not "focus still where it was": a view that wraps its screen in the
/// overlay only while the dialog is open (chat's Create channel) moves every
/// path under it as it opens, so the opener's host focus handle dies with
/// its path in that frame, and the window takes the keys back to its own
/// root before this runs (`WindowRoot::focus_lost`, shell/layers/root.rs) —
/// focus moved, and nothing in the dialog has it. Nothing inside the dialog
/// tracks `entry` as well: gpui's dispatch tree keys a handle to the last
/// element that tracks it, so the trap would contain no focused node and
/// Tab would leave the dialog.
pub(crate) fn dialog_entry(entry: &FocusHandle, window: &mut Window, cx: &mut App) {
    let entry = entry.clone();
    window.defer(cx, move |window, cx| {
        if !entry.contains_focused(window, cx) {
            window.focus(&entry, cx);
            tab(Some(&entry), true, window, cx);
        }
    });
}

/// Tab's step (`forward`) or Shift-Tab's, as the kit's root takes it for a
/// key press (gpui-base `Root::on_action_tab`): one stop on, and while that
/// leaves `trap`, on round the window's stops until the keys are back in
/// it; with no stop in the trap they stay where they were. A bare
/// `focus_next` is not Tab: gpui orders stops as they paint, a deferred
/// popover's after everything painted in place, so one step from a dialog's
/// trap lands on whatever is drawn after its opener, outside the dialog.
pub(crate) fn tab(trap: Option<&FocusHandle>, forward: bool, window: &mut Window, cx: &mut App) {
    let step = |window: &mut Window, cx: &mut App| match forward {
        true => window.focus_next(cx),
        false => window.focus_prev(cx),
    };
    let start = window.focused(cx);
    step(window, cx);
    let Some(trap) = trap else { return };
    let first = window.focused(cx);
    while !trap.contains_focused(window, cx) {
        step(window, cx);
        if window.focused(cx) == first {
            // round once, and no stop in the trap
            if let Some(start) = &start {
                window.focus(start, cx);
            }
            return;
        }
    }
}

/// Where focus goes when a dialog closes: back to `opener`, what held it as
/// the dialog opened, if focus is in the dialog (`entry`, its trap) as the
/// frame that drops it renders. Focus the view moved elsewhere itself stays
/// there.
///
/// Decided in that render, not after its draw: the draw drops the focused
/// element with the dialog, and a window that takes the keys back to its
/// own root when its focused element vanishes (`WindowRoot::focus_lost`,
/// shell/layers/root.rs) has them on the root by then. Focus moved to the
/// opener before the draw compares focus paths is never lost.
pub(crate) fn dialog_exit(
    entry: &FocusHandle,
    opener: WeakFocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    if entry.contains_focused(window, cx)
        && let Some(opener) = opener.upgrade()
    {
        window.focus(&opener, cx);
    }
}

impl ViewTree {
    /// The host received a real press or key aimed at this tree: a click,
    /// an auxiliary click, a link's press, a key down but Escape, typing in
    /// a native field or editor, assistive technology's press. Nothing a
    /// view sends, no timer, no answer, no hover or scroll comes here.
    pub(crate) fn activate(&self) {
        self.activation.set(Some(std::time::Instant::now()));
    }

    /// The activation, taken: the seat moves it to the guest.
    pub(crate) fn take_activation(&self) -> Option<std::time::Instant> {
        self.activation.take()
    }

    /// See `keys_grant`: the seat mirrors its `keys_free` here.
    pub(crate) fn set_keys_grant(&mut self, keys_grant: bool) {
        self.keys_grant = keys_grant;
    }

    pub(crate) fn with_keys_grant(mut self, keys_grant: bool) -> Self {
        self.keys_grant = keys_grant;
        self
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
            // Tab's own move: through the window's Tab stops, kept in a
            // focus trap, never a walk of this tree's own
            C::FocusPrevious => {
                let trap = gpui_kit::base::active_focus_trap(window, cx);
                tab(trap.as_ref(), false, window, cx)
            }
            C::FocusNext => {
                let trap = gpui_kit::base::active_focus_trap(window, cx);
                tab(trap.as_ref(), true, window, cx)
            }
            C::EditorAction { ref target, .. }
            | C::Focus { ref target }
            | C::CursorFront { ref target }
            | C::CursorEnd { ref target }
            | C::Cursor { ref target, .. }
            | C::SelectAll { ref target }
            | C::Select { ref target, .. } => self.input_command(target, &command, window, cx)?,
            C::Snap { target, x, y } => {
                self.scroll_command(&target, ScrollRequest::Relative(x, y), cx)?
            }
            C::SnapEnd { target } => self.scroll_command(&target, ScrollRequest::End, cx)?,
            C::ScrollTo { target, x, y } => {
                self.scroll_command(&target, ScrollRequest::Absolute(x, y), cx)?
            }
            C::ScrollBy { target, x, y } => {
                self.scroll_command(&target, ScrollRequest::By(x, y), cx)?
            }
        }
        Ok(wire::encode(&()))
    }

    /// Whether the keyboard is in the node at `target`, a whole authored
    /// path.
    #[cfg(test)]
    pub(super) fn target_focused(
        &self,
        target: &[wire::ElementIdWire],
        window: &Window,
        cx: &App,
    ) -> bool {
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

    /// Runs a focus, cursor, selection or editor command on the node at
    /// `target`: a whole authored path, as the guest SDK names it from the
    /// frame it lowered, so two scopes holding one id are two targets. A
    /// node that cannot take the command answers so.
    pub(super) fn input_command(
        &mut self,
        target: &[wire::ElementIdWire],
        command: &wire::WidgetCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        use wire::WidgetCommand as C;
        if matches!(command, C::Focus { .. }) {
            // the container that owns the path: an id-less one sits on its
            // nearest named ancestor's path and is not the node it names
            let mut kind = None;
            walk_authored_paths(&self.root, &mut Vec::new(), &mut |node, path| {
                if path == target
                    && matches!(
                        node,
                        wire::Node::Container(view_wire::ContainerNode { id: Some(_), .. })
                    )
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
        Err("the target is no container, field or editor: it takes no focus or caret".into())
    }

    pub(super) fn scroll_command(
        &mut self,
        target: &[wire::ElementIdWire],
        request: ScrollRequest,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(handle) = self.scrolls.get(target) else {
            return Err("the target is no scroller with an id".into());
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
        Ok(())
    }
}
