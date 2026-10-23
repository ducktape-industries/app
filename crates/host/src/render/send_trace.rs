//! Painted authored-node metadata for the opt-in send trace.

use super::*;

struct Recorded {
    element: AnyElement,
    key: crate::perf::Key,
    revision: u64,
    path: AuthoredPath,
    opacity: Option<f32>,
    disabled: Option<bool>,
    selected: Option<bool>,
    expanded: Option<bool>,
    busy: bool,
    click_handler: Option<u32>,
    key_handler: Option<u32>,
    submit_handler: Option<u32>,
}

pub(super) fn recorded(
    element: AnyElement,
    key: crate::perf::Key,
    revision: u64,
    path: &AuthoredPath,
    node: &wire::Node,
    styles: &wire::Styles,
) -> AnyElement {
    let aria = node
        .interactivity()
        .map(|interactivity| &interactivity.aria);
    let interactivity = node.interactivity();
    let submit_handler = match node {
        wire::Node::Field { on_submit, .. } => *on_submit,
        _ => None,
    };
    Recorded {
        element,
        key,
        revision,
        path: path.clone(),
        opacity: node.style().and_then(|style| styles[style].opacity),
        disabled: aria.and_then(|aria| aria.disabled),
        selected: aria.and_then(|aria| aria.selected),
        expanded: aria.and_then(|aria| aria.expanded),
        busy: aria.is_some_and(|aria| aria.busy),
        click_handler: interactivity.and_then(|interactivity| interactivity.on_click),
        key_handler: interactivity.and_then(|interactivity| interactivity.on_key_down),
        submit_handler,
    }
    .into_any_element()
}

impl IntoElement for Recorded {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Recorded {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.element.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(window, cx);
        let visible = window.content_mask().bounds.intersect(&bounds);
        let has_area = visible.size.width > px(0.) && visible.size.height > px(0.);
        if !has_area {
            return;
        }
        tracing::debug!(
            target: "ducktape::send_trace",
            stage = "node_painted",
            elapsed_us = crate::send_trace::elapsed_us(),
            key = ?self.key,
            revision = self.revision,
            path = ?self.path,
            style_opacity = ?self.opacity,
            disabled = ?self.disabled,
            selected = ?self.selected,
            expanded = ?self.expanded,
            busy = self.busy,
            click_handler = ?self.click_handler,
            key_handler = ?self.key_handler,
            submit_handler = ?self.submit_handler,
        );
    }
}
