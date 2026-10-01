//! `Node::Editor`: a mount of the native `TextEditor` over a guest-owned
//! document, and the rows a growing field shows before it scrolls.

use super::*;
use crate::render::native_id;

/// A mounted `Node::Editor`, kept for as long as its path stays mounted.
pub(super) struct EditorMount {
    pub(super) view: EditorView,
    pub(super) _subscription: Subscription,
}

/// `TextEditor` emits when the `EditorStore` has queued guest events (and
/// whether the input that queued them activates the view): take them and
/// emit them as this tree's own.
pub(super) fn drain_editor(store: &crate::editor::wire::EditorStore, cx: &mut Context<ViewTree>) {
    for event in store.drain() {
        cx.emit(event);
    }
}

/// The native editor drawing a mounted `Node::Editor`. One kind is left
/// since the block editor went (549f8427); the enum is its seam.
pub(super) enum EditorView {
    Text(Entity<crate::editor::wire::TextEditor>),
}

impl EditorView {
    pub(super) fn sync(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| editor.sync(window, cx)),
        }
    }

    /// Whether this editor takes the box it is given or takes the room its
    /// words need. The field's own element decides its height, so the node's
    /// answer has to reach it — a box told to shrink around an element that
    /// still asks for all of its parent's height shrinks around nothing.
    pub(super) fn fills(&self, fills: bool, cap: Option<usize>, cx: &mut App) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| editor.set_fills(fills, cap, cx)),
        }
    }

    /// The node's mapping, onto the field the editor draws: the node this
    /// mount announces is a wrapper, not the text.
    pub(super) fn announce(&self, accessible: Accessible, cx: &mut App) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| editor.set_accessible(accessible, cx)),
        }
    }

    pub(super) fn widget_command(
        &self,
        command: &wire::WidgetCommand,
        window: &mut Window,
        cx: &mut App,
    ) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| {
                editor.widget_command(command, window, cx);
            }),
        }
    }

    /// The guest echoes which editor held focus when the frame was built, so
    /// the field it named takes the caret back.
    pub(super) fn restore_focus(
        &self,
        path: &[wire::ElementIdWire],
        window: &mut Window,
        cx: &mut App,
    ) {
        let Self::Text(view) = self;
        let focus = wire::WidgetCommand::Focus {
            target: path.to_vec(),
        };
        view.update(cx, |editor, cx| {
            editor.widget_command(&focus, window, cx);
        });
    }

    pub(super) fn is_focused(&self, window: &Window, cx: &App) -> bool {
        match self {
            Self::Text(view) => view.read(cx).is_focused(window, cx),
        }
    }

    pub(super) fn element(&self) -> AnyElement {
        match self {
            Self::Text(view) => view.clone().into_any_element(),
        }
    }
}

impl ViewTree {
    pub(super) fn editor(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Editor {
            id,
            style,
            document,
            ..
        } = node
        else {
            unreachable!()
        };
        let Some(store) = self.editor_store.clone() else {
            return div().child("Editor host is unavailable").into_any_element();
        };
        let path = self.authored_path.clone();
        debug_assert_eq!(path.last(), Some(id));
        if !self.editors.contains_key(&path) {
            let events = store.clone();
            let (view, subscription) = {
                let view = cx.new(|cx| {
                    crate::editor::wire::TextEditor::new(path.clone(), store, window, cx)
                });
                let subscription = cx.subscribe(&view, move |this, _, activates: &bool, cx| {
                    if *activates {
                        this.activate();
                    }
                    drain_editor(&events, cx)
                });
                (EditorView::Text(view), subscription)
            };
            self.editors.insert(
                path.clone(),
                EditorMount {
                    view,
                    _subscription: subscription,
                },
            );
        }
        let editor = self.editors.get(&path).expect("editor inserted");
        // A height the guest gave is a box to fill; none (a composer's
        // `min_h`..`max_h`) is a field as tall as its lines, which the
        // field's own auto-grow reports up to the parent that waits on it.
        let cap = field_rows(style);
        editor.view.fills(style.size.height.is_some(), cap, cx);
        editor.view.announce(accessible(node), cx);
        editor.view.sync(window, cx);
        if self.presentation.editors.remove(&path).as_ref() == Some(document) {
            editor.view.restore_focus(&path, window, cx);
        }
        let view = editor.view.element();
        let mut element = div().relative().id(native_id(id));
        *element.style() = style.clone();
        // the view's box is the field's, border and padding: it wears the
        // ring, not the text inside the padding
        let focused = editor.view.is_focused(window, cx);
        crate::a11y::around_field(element, focused, crate::a11y::ink(cx))
            .child(view)
            .child(self.measure(&path, cx))
            .into_any_element()
    }
}

/// The text size `field_rows` assumes when the node's style sets none.
/// Note: gpui's own default is 16px (`rems(1.)`), so a field styled without
/// a size is counted at a smaller face than it is drawn with.
const FALLBACK_FIELD_TEXT_PX: f32 = 14.;

/// gpui's default line height as a multiple of the text size (`phi()`,
/// 1.618034, to the three places the literal always had).
const FIELD_LINE_HEIGHT: f32 = 1.618;

/// How many lines a field that grows with its words shows before it scrolls:
/// what its node's `max_h` holds, less its vertical padding, at the field's
/// line height.
fn field_rows(style: &gpui_kit::StyleRefinement) -> Option<usize> {
    use gpui_kit::{AbsoluteLength, DefiniteLength, Length};
    let pixels = |length: Option<DefiniteLength>| match length {
        Some(DefiniteLength::Absolute(AbsoluteLength::Pixels(pixels))) => f32::from(pixels),
        _ => 0.,
    };
    let Some(Length::Definite(max)) = style.max_size.height else {
        return None;
    };
    let pad = pixels(style.padding.top) + pixels(style.padding.bottom);
    let size = match style.text.font_size {
        Some(AbsoluteLength::Pixels(size)) => f32::from(size),
        _ => FALLBACK_FIELD_TEXT_PX,
    };
    let rows = (pixels(Some(max)) - pad) / (size * FIELD_LINE_HEIGHT);
    Some((rows.floor() as usize).max(1))
}
