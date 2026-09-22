//! Native tooltip passes retain the source view slot and event authority.
use super::*;
use gpui_kit::{ContentMask, Subscription, WeakEntity};
use std::{cell::Cell, rc::Rc};

pub(super) type SlotMask = Rc<Cell<ContentMask<Pixels>>>;

pub(super) fn build(
    parent: WeakEntity<ViewTree>,
    content: wire::Node,
    cx: &mut App,
) -> gpui_kit::AnyView {
    let mask = parent
        .upgrade()
        .map(|parent| parent.read(cx).slot_mask.clone())
        .unwrap_or_default();
    let child = cx.new(|_| ViewTree::new(content));
    cx.new(|cx| {
        let subscription = cx.subscribe(&child, move |_, source, event: &wire::Event, cx| {
            let source = source.read(cx);
            let handler = source.user_activation.get();
            let activated = source.take_user_activation(event).is_some();
            let _ = parent.update(cx, |parent, cx| {
                if activated {
                    parent.user_activation.set(handler);
                }
                cx.emit(event.clone());
            });
        });
        TooltipHost {
            child,
            mask,
            _subscription: subscription,
        }
    })
    .into()
}

struct TooltipHost {
    child: Entity<ViewTree>,
    mask: SlotMask,
    _subscription: Subscription,
}
impl Render for TooltipHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Contained {
            child: self.child.clone().into_any_element(),
            mask: self.mask.clone(),
        }
    }
}

pub(super) struct Contained {
    pub(super) child: AnyElement,
    pub(super) mask: SlotMask,
}
impl IntoElement for Contained {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Contained {
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
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(self.mask.get()), |window| {
            self.child.prepaint(window, cx);
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(self.mask.get()), |window| self.child.paint(window, cx));
    }
}
