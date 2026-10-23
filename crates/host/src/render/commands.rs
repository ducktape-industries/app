//! What a view asks of the mounted tree between frames: widget commands
//! (`host.widget`: focus, cursor, scroll, a field's edit) on the node at
//! the whole authored path they name, the walk that names those paths, the
//! user-activation mark an event may spend, the entry a dialog gives the
//! keyboard when it opens, and the way back when it closes.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum ScrollRequest {
    Relative(f32, f32),
    Absolute(f32, f32),
    By(f32, f32),
    End,
}

pub(super) fn walk_authored_paths(
    node: &wire::Node,
    row: Option<usize>,
    path: &mut AuthoredPath,
    visit: &mut impl FnMut(&wire::Node, &AuthoredPath),
) {
    let entered_scope = crate::render::enter_scope(node, row, path);
    visit(node, path);
    for (at, child) in node.children().iter().enumerate() {
        walk_authored_paths(child, wire::identity::row(node, at), path, visit);
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
///
/// On a touch device the keys stop at the trap itself: the first stop is
/// often a field, and a focused field brings the soft keyboard up over a
/// sheet nobody has touched yet. The phone's sheets focus nothing as they
/// open (the Expo app's `Field` takes `autoFocus` only where a screen asks);
/// a view that wants its field focused asks for it.
pub fn dialog_entry(entry: &FocusHandle, window: &mut Window, cx: &mut App) {
    let entry = entry.clone();
    window.defer(cx, move |window, cx| {
        if !entry.contains_focused(window, cx) {
            window.focus(&entry, cx);
            if !crate::contract::os().facts().touch_first {
                tab(Some(&entry), true, window, cx);
            }
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
pub fn tab(trap: Option<&FocusHandle>, forward: bool, window: &mut Window, cx: &mut App) {
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
pub fn dialog_exit(
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
    pub fn activate(&self) {
        self.activation.set(Some(rt::time::Instant::now()));
    }

    /// The activation, taken: the seat moves it to the view.
    pub fn take_activation(&self) -> Option<rt::time::Instant> {
        self.activation.take()
    }

    /// See `keys_grant`: the seat mirrors its `keys_free` here.
    pub fn set_keys_grant(&mut self, keys_grant: bool) {
        self.keys_grant = keys_grant;
    }

    pub fn with_keys_grant(mut self, keys_grant: bool) -> Self {
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
        match command {
            C::FocusHandle { handle } => {
                let focus = self
                    .view_focus_targets
                    .get(&handle)
                    .ok_or_else(|| "focus handle is not mounted".to_string())?
                    .clone();
                focus.focus(window, cx);
            }
            C::FocusPrevious => {
                let trap = gpui_kit::base::active_focus_trap(window, cx);
                tab(trap.as_ref(), false, window, cx)
            }
            C::FocusNext => {
                let trap = gpui_kit::base::active_focus_trap(window, cx);
                tab(trap.as_ref(), true, window, cx)
            }
            C::Replace { ref target, .. }
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
        self.fields
            .get(target)
            .is_some_and(|field| field.is_focused(window, cx))
    }

    pub(super) fn input_command(
        &mut self,
        target: &[wire::ElementIdWire],
        command: &wire::WidgetCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        use wire::WidgetCommand as C;
        if matches!(command, C::Focus { .. }) {
            let mut kind = None;
            walk_authored_paths(&self.root, None, &mut Vec::new(), &mut |node, path| {
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
        if let Some(field) = self.fields.get_mut(target) {
            let event = match command {
                C::Focus { .. } => {
                    field.focus(window, cx);
                    None
                }
                C::Replace { .. } => field.ask(command.clone(), window, cx),
                _ => field.cursor_command(command, window, cx),
            };
            if let Some(event) = event {
                cx.emit(event);
            }
            return Ok(());
        }
        Err("the target is no container or field: it takes no focus, edit or caret".into())
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
