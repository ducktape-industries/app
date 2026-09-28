//! A RichText's clickable ranges as assistive technology meets them: each
//! a `Link` child of the text's node, named by its words, whose press is
//! the range's click (AX-117). gpui's `InteractiveText` builds the text's
//! node; [`Linked`] wraps it, adds the links as synthetic children at
//! prepaint, and registers their presses with the window at paint, the
//! first moment their ids and the window meet.
use super::*;
use gpui_kit::accesskit::{Action, Node, NodeId, Role};
use gpui_kit::{A11ySubtreeBuilder, WeakEntity};

pub(super) struct Linked {
    text: InteractiveText,
    /// each clickable range's words, in range order
    words: Vec<String>,
    handler: u32,
    view: WeakEntity<ViewTree>,
    /// each link's range index and node id, as prepaint made them
    links: Vec<(u32, NodeId)>,
}

impl Linked {
    pub(super) fn new(
        text: InteractiveText,
        words: &str,
        ranges: &[std::ops::Range<usize>],
        handler: u32,
        view: WeakEntity<ViewTree>,
    ) -> Self {
        let words = ranges
            .iter()
            .map(|range| words.get(range.clone()).unwrap_or_default().to_owned())
            .collect();
        Self {
            text,
            words,
            handler,
            view,
            links: Vec::new(),
        }
    }
}

impl IntoElement for Linked {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Linked {
    type RequestLayoutState = <InteractiveText as Element>::RequestLayoutState;
    type PrepaintState = <InteractiveText as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.text.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.text.source_location()
    }

    fn a11y_role(&self) -> Option<Role> {
        self.text.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut Node) {
        self.text.write_a11y_info(node);
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text
            .prepaint(id, inspector_id, bounds, state, window, cx)
    }

    fn a11y_synthetic_children(
        &mut self,
        _: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        for (index, words) in (0u32..).zip(&self.words) {
            let id = builder.synthetic_node_id(("link", index));
            let mut link = Node::new(Role::Link);
            link.set_label(words.clone());
            link.add_action(Action::Click);
            if builder.push_child(id, link) {
                self.links.push((index, id));
            }
        }
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .paint(id, inspector_id, bounds, state, prepaint, window, cx);
        // a press from assistive technology grants no user activation
        for (index, node) in self.links.drain(..) {
            let (view, handler) = (self.view.clone(), self.handler);
            window.on_a11y_action(node, Action::Click, move |_, _, cx| {
                let _ = view.update(cx, |_, cx| cx.emit(wire::Event::Select { handler, index }));
            });
        }
    }
}
