//! Where the inspector's nodes were last drawn while the door serves, kept
//! in each element's own state, and a pointer's wheel at them.

use super::natives::{self, Refused};
use crate::render::AuthoredPath;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, Bounds, Element, ElementId, EntityId, Global,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, Modifiers,
    Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, TouchPhase, Window, point,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use view_wire::ElementIdWire;

#[derive(Clone, Debug)]
pub struct Shown {
    pub window: AnyWindowHandle,
    pub hitbox: Hitbox,
    pub bounds: Bounds<Pixels>,
    pub mask: Bounds<Pixels>,
}

impl Shown {
    pub fn new(bounds: Bounds<Pixels>, window: &mut Window) -> Self {
        Shown {
            window: window.window_handle(),
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            bounds,
            mask: window.content_mask().bounds,
        }
    }

    pub fn visible(&self) -> Bounds<Pixels> {
        self.bounds.intersect(&self.mask)
    }

    pub fn area(&self) -> f32 {
        let size = self.visible().size;
        f32::from(size.width.max(Pixels::ZERO)) * f32::from(size.height.max(Pixels::ZERO))
    }

    pub fn aim(&self, id: &str, window: &Window) -> Result<Point<Pixels>, Refused> {
        if self.area() <= 0. {
            return Err((
                409,
                format!("{id} is not shown: it is clipped or scrolled away"),
            ));
        }
        let at = self.visible().center();
        match self.hitbox.is_hovered_at(at, window) {
            true => Ok(at),
            false => Err((
                409,
                format!("{id} is covered: a pointer at its middle would reach what is over it"),
            )),
        }
    }

    pub fn toward(&self) -> Point<Pixels> {
        let axis = |start: Pixels, length: Pixels, view: Pixels, room: Pixels| match start < view
            || length > room
        {
            true => view - start,
            false => (view + room - start - length).min(Pixels::ZERO),
        };
        let (bounds, mask) = (self.bounds, self.mask);
        point(
            axis(
                bounds.origin.x,
                bounds.size.width,
                mask.origin.x,
                mask.size.width,
            ),
            axis(
                bounds.origin.y,
                bounds.size.height,
                mask.origin.y,
                mask.size.height,
            ),
        )
    }
}

#[derive(Default)]
pub struct Views {
    drawn: HashMap<(EntityId, AuthoredPath), Weak<RefCell<Shown>>>,
}

impl Global for Views {}

pub fn recorded(element: AnyElement, tree: EntityId, path: &[ElementIdWire]) -> AnyElement {
    Recorded {
        element,
        key: (tree, path.to_vec()),
    }
    .into_any_element()
}

pub fn view(tree: EntityId, path: &[ElementIdWire], cx: &App) -> Option<Shown> {
    let kept = cx
        .try_global::<Views>()?
        .drawn
        .get(&(tree, path.to_vec()))?
        .upgrade()?;
    let shown = kept.borrow().clone();
    Some(shown)
}

pub fn wheel(at: Point<Pixels>, by: Point<Pixels>, window: &mut Window, cx: &mut App) {
    window.dispatch_event(
        PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(by),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        }),
        cx,
    );
    redraw(window, cx);
}

pub fn redraw(window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
}

pub fn seen(window: AnyWindowHandle, cx: &mut App) -> Vec<[f32; 4]> {
    let mut kept: Vec<Shown> = natives::drawn(cx)
        .iter()
        .map(|drawn| drawn.borrow().shown.clone())
        .collect();
    if cx.has_global::<Views>() {
        let views = cx.global_mut::<Views>();
        views.drawn.retain(|_, shown| shown.strong_count() > 0);
        kept.extend(
            views
                .drawn
                .values()
                .filter_map(Weak::upgrade)
                .map(|shown| shown.borrow().clone()),
        );
    }
    let mut seen: Vec<[f32; 4]> = kept
        .iter()
        .filter(|shown| shown.window == window)
        .map(|shown| {
            let bounds = shown.bounds;
            [
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            ]
            .map(f32::from)
        })
        .collect();
    seen.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    seen
}

struct Recorded {
    element: AnyElement,
    key: (EntityId, AuthoredPath),
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
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.prepaint(window, cx);
        let shown = Shown::new(bounds, window);
        let name = ElementId::Name(format!("inspected:{:?}", self.key.1).into());
        let key = &self.key;
        window.with_global_id(name, |global, window| {
            window.with_element_state::<Rc<RefCell<Shown>>, _>(global, |kept, _| {
                let kept = match kept {
                    Some(kept) => {
                        *kept.borrow_mut() = shown;
                        kept
                    }
                    None => {
                        let kept = Rc::new(RefCell::new(shown));
                        if cx.has_global::<Views>() {
                            cx.global_mut::<Views>()
                                .drawn
                                .insert(key.clone(), Rc::downgrade(&kept));
                        }
                        kept
                    }
                };
                ((), kept)
            })
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(window, cx);
    }
}
