//! A RichText's clickable ranges as the keyboard and assistive technology
//! meet them. The text is one Tab stop, a box the host holds the focus
//! handle of; while it has the keys the arrows pick a link and Enter is
//! the picked range's click. Each range is a `Link` child of the box,
//! named by its words, whose press is the range's click (AX-117): the
//! picked one, while the box has the keys, is a real element that claims to
//! be the active descendant (gpui takes the claim only from an element,
//! and only under the focused node); the rest are synthetic children,
//! which [`Linked`] presses for assistive technology, registering them
//! with the window at paint, the first moment their ids and the window
//! meet.
use super::*;
use gpui_kit::accesskit::{Action, Node, NodeId, Role};
use gpui_kit::{
    A11ySubtreeBuilder, AccessibleAction, CursorStyle, DispatchPhase, Hitbox, WeakEntity,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What the host keeps for a text with links between frames: the box's
/// focus handle, and which link the arrows picked.
pub struct Links {
    pub focus: FocusHandle,
    pub picked: Rc<Cell<usize>>,
    pub down: Down,
    /// Set by the render that draws it; a render that does not drops it.
    pub drawn: bool,
}

pub(super) type Press = Rc<dyn Fn(usize, &mut App)>;

pub type Down = Rc<Cell<Option<(usize, Point<Pixels>)>>>;

pub(super) struct Linking {
    handler: u32,
    press: Press,
    ranges: Vec<std::ops::Range<usize>>,
    down: Down,
    ids: Rc<RefCell<Vec<(u32, NodeId)>>>,
    pub(super) picked: Option<usize>,
}

impl ViewTree {
    pub(super) fn linked_box(
        &mut self,
        id: &ElementId,
        text: &str,
        ranges: &[std::ops::Range<usize>],
        handler: u32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, Linking) {
        let mut key = self.authored_path.clone();
        if is_host_id(id) {
            key.push(
                wire::ElementIdWire::from_gpui(id.clone())
                    .expect("a host id is one name over a code location"),
            );
        }
        let words: Vec<String> = ranges
            .iter()
            .map(|range| text.get(range.clone()).unwrap_or_default().to_owned())
            .collect();
        let view = cx.entity().downgrade();
        let press: Press = {
            let view = view.clone();
            Rc::new(move |index, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.activate();
                    cx.emit(wire::Event::Select {
                        handler,
                        index: index as u32,
                    });
                });
            })
        };
        let ids: Rc<RefCell<Vec<(u32, NodeId)>>> = Rc::default();
        let links = self.links.entry(key).or_insert_with(|| Links {
            focus: cx.focus_handle().tab_stop(true),
            picked: Rc::default(),
            down: Rc::default(),
            drawn: true,
        });
        links.drawn = true;
        let down = links.down.clone();
        let last = words.len() - 1;
        let focused = links.focus.is_focused(window);
        links.picked.set(match focused {
            true => links.picked.get().min(last),
            false => 0,
        });
        let picked = links.picked.get();
        let pick = links.picked.clone();
        let mut element = crate::a11y::keyboard(
            div()
                .id(id.clone())
                .track_focus(&links.focus)
                .role(Role::Group)
                .aria_label(text.to_owned())
                .a11y_synthetic_children({
                    let (words, ids) = (words.clone(), ids.clone());
                    move |tree: &mut A11ySubtreeBuilder| {
                        for (index, words) in words.iter().enumerate() {
                            if focused && index == picked {
                                continue;
                            }
                            let id = tree.synthetic_node_id(("link", index));
                            let mut link = Node::new(Role::Link);
                            link.set_label(words.clone());
                            link.add_action(Action::Click);
                            if tree.push_child(id, link) {
                                ids.borrow_mut().push((index as u32, id));
                            }
                        }
                    }
                })
                .on_key_down({
                    let (view, press) = (view.clone(), press.clone());
                    move |event: &KeyDownEvent, _, cx| {
                        let at = pick.get();
                        let to = match event.keystroke.key.as_str() {
                            "left" | "up" => at.saturating_sub(1),
                            "right" | "down" => (at + 1).min(last),
                            "home" => 0,
                            "end" => last,
                            "enter" | "space" => {
                                cx.stop_propagation();
                                press(at, cx);
                                return;
                            }
                            _ => return,
                        };
                        cx.stop_propagation();
                        pick.set(to);
                        let _ = view.update(cx, |_, cx| cx.notify());
                    }
                }),
            crate::a11y::ink(cx),
        );
        if focused {
            element = element.child(
                div()
                    .id(("link", picked as u64))
                    .absolute()
                    .size_full()
                    .role(Role::Link)
                    .aria_label(words[picked].clone())
                    .aria_active_descendant()
                    .on_a11y_action(AccessibleAction::Click, move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.activate();
                            cx.emit(wire::Event::Select {
                                handler,
                                index: picked as u32,
                            })
                        });
                    }),
            );
        }
        (
            element,
            Linking {
                handler,
                press,
                ranges: ranges.to_vec(),
                down,
                ids,
                picked: focused.then_some(picked),
            },
        )
    }
}

pub(super) struct Linked {
    text: InteractiveText,
    layout: TextLayout,
    ranges: Vec<std::ops::Range<usize>>,
    down: Down,
    press: Press,
    handler: u32,
    view: WeakEntity<ViewTree>,
    links: Rc<RefCell<Vec<(u32, NodeId)>>>,
}

impl Linked {
    pub(super) fn new(
        text: InteractiveText,
        layout: TextLayout,
        linking: Linking,
        view: WeakEntity<ViewTree>,
    ) -> Self {
        Self {
            text,
            layout,
            ranges: linking.ranges,
            down: linking.down,
            press: linking.press,
            handler: linking.handler,
            view,
            links: linking.ids,
        }
    }

    fn listen(&self, hitbox: &Hitbox, window: &mut Window) {
        if range_at(&self.layout, &self.ranges, window.mouse_position()).is_some() {
            window.set_cursor_style(CursorStyle::PointingHand, hitbox);
        }
        let (layout, ranges, down) = (self.layout.clone(), self.ranges.clone(), self.down.clone());
        let held = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
            if (phase, event.button) != (DispatchPhase::Bubble, MouseButton::Left) {
                return;
            }
            if held.is_hovered(window) {
                let at = range_at(&layout, &ranges, event.position);
                down.set(at.map(|index| (index, event.position)));
            }
        });
        let (layout, ranges, down) = (self.layout.clone(), self.ranges.clone(), self.down.clone());
        let (held, press) = (hitbox.clone(), self.press.clone());
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if (phase, event.button) != (DispatchPhase::Bubble, MouseButton::Left) {
                return;
            }
            let Some((index, from)) = down.take() else {
                return;
            };
            let released = held
                .is_hovered(window)
                .then(|| range_at(&layout, &ranges, event.position))
                .flatten();
            if released != Some(index) {
                return;
            }
            if !super::dragged_over_words(from, event.position, window, cx) {
                press(index, cx);
            }
        });
    }
}

fn range_at(
    layout: &TextLayout,
    ranges: &[std::ops::Range<usize>],
    position: Point<Pixels>,
) -> Option<usize> {
    let index = layout.index_for_position(position).ok()?;
    ranges.iter().position(|range| range.contains(&index))
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
        None
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
        self.listen(prepaint, window);
        for (index, node) in self.links.borrow_mut().drain(..) {
            let (view, handler) = (self.view.clone(), self.handler);
            window.on_a11y_action(node, Action::Click, move |_, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.activate();
                    cx.emit(wire::Event::Select { handler, index })
                });
            });
        }
    }
}
