//! Text and RichText nodes. A RichText paragraph is wrapped in
//! `RichParagraph`, which registers it with gpui-base's cross-paragraph text
//! selection so a drag selects in reading order and copy yields the source
//! text; `DRAG_CLIP` keeps a drag inside the clip it began in.
use super::*;
use crate::render::native_id;

pub(super) mod links;

/// The content the guest built for the rich tooltip `request` over
/// `character_index`, once it has answered for that character.
fn rich_tooltip_content(
    tooltips: &HashMap<u32, TooltipContent>,
    request: u32,
    character_index: u32,
) -> Option<Tree> {
    let held = tooltips.get(&request)?;
    (held.character_index == Some(character_index)).then(|| Tree {
        root: (*held.content).clone(),
        styles: held.styles.clone(),
    })
}

thread_local! {
    /// The clip a text drag began in, from the press to the release. While
    /// it is held only paragraphs painted in that clip — the list or pane
    /// the drag started in — take part, so a pointer that wanders into the
    /// thread or the sidebar cannot pull their text into the selection.
    static DRAG_CLIP: std::cell::Cell<Option<Bounds<Pixels>>> = const { std::cell::Cell::new(None) };
    /// Where the last left press on a paragraph went down: a link's click
    /// says which range it was, not where the press began.
    static PRESS: std::cell::Cell<Option<Point<Pixels>>> = const { std::cell::Cell::new(None) };
    /// The window where a press that went down on a paragraph has been
    /// released, and the selection it left is still to be weighed by the
    /// next frame's paragraphs, unless a new press comes first
    /// (`RichParagraph::paint`).
    static SETTLING: std::cell::Cell<Option<gpui_kit::WindowId>> = const { std::cell::Cell::new(None) };
}

/// How far a press may travel and still be a click: gpui's own threshold
/// for a drag (`DRAG_THRESHOLD`, private to its `div.rs`).
const DRAG_THRESHOLD: f64 = 2.;

/// Whether a press at `down` released at `up` was a drag that selected
/// words: then the release pressed nothing, whatever box it landed on.
/// Both halves count. A press that drifted no farther than a drag goes (a
/// trackpad click's `mouseDragged`, fractional coordinates on macOS) is a
/// click whatever the selection holds: until the next frame paints, a
/// paragraph the selection just took in copies its whole text, however
/// little of it the drift covered. A drag over no words is a click too.
pub(super) fn dragged_over_words(
    down: Point<Pixels>,
    up: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    (up - down).magnitude() > DRAG_THRESHOLD
        && !gpui_kit::base::TextSelection::selected_text(window, cx).is_empty()
}

/// Whether a paragraph painted in `clip` takes part in the selection while
/// a drag that began in `drag` is held.
pub(super) fn joins_drag(drag: Option<Bounds<Pixels>>, clip: Bounds<Pixels>) -> bool {
    drag.is_none_or(|drag| drag == clip)
}

/// Redraws `window` when the selection `handle` shows really changes.
/// gpui-base's sweep clears every participant a frame did not register — a
/// cached view's paragraphs register none — and its clear says
/// `SelectionChanged(None)` even when nothing was selected. Refreshing on that re-rendered every cached view, every
/// frame: a desk of heavy views drew all of them in full on each switch.
fn refresh_on_change(
    handle: &gpui_kit::base::TextSelectionHandle,
    window: &Window,
    cx: &mut App,
) -> Subscription {
    let window = window.window_handle();
    let mut shown = None;
    handle.subscribe(
        move |event, cx| {
            if let gpui_kit::base::TextSelectionEvent::SelectionChanged(snapshot) = event
                && *snapshot != shown
            {
                shown = *snapshot;
                _ = window.update(cx, |_, window, _| window.refresh());
            }
        },
        cx,
    )
}

pub(super) struct RichSelection {
    pub(super) handle: gpui_kit::base::TextSelectionHandle,
    pub(super) _refresh: Subscription,
}

// Layout and hit-testing stay native. One participant receives every span's
// measured glyph run so copying concatenates source text, never visual padding.
pub(super) struct RichParagraph {
    pub(super) id: ElementId,
    pub(super) content: AnyElement,
    pub(super) text: SharedString,
    pub(super) layout: TextLayout,
    pub(super) active_handle:
        std::rc::Rc<std::cell::RefCell<Option<gpui_kit::base::TextSelectionHandle>>>,
    pub(super) selection: std::rc::Rc<std::cell::RefCell<Option<std::ops::Range<usize>>>>,
    /// The view's paint-order counter: each paragraph takes the next, so a
    /// drag selects what lies between its ends in reading order, not every
    /// line in the window between their heights.
    pub(super) order: std::rc::Rc<std::cell::Cell<u64>>,
}

impl IntoElement for RichParagraph {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for RichParagraph {
    type RequestLayoutState = ();
    type PrepaintState = (gpui_kit::Hitbox, Bounds<Pixels>);
    // No id of its own: the content carries `id`, so what is inside is
    // filed under the one id the view gave (or the host numbered), and the
    // selection state below is keyed under that same id.
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.content.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        // Register the containing hitbox before link hitboxes, so selectable
        // paragraph geometry cannot cover its own interactive spans.
        self.content.prepaint(window, cx);
        let clip = window.content_mask().bounds;
        let order = self.order.get();
        self.order.set(order + 1);
        let joins = joins_drag(DRAG_CLIP.get(), clip);
        let text = self.text.clone();
        let layout_bounds = self.layout.bounds();
        let active = self.active_handle.clone();
        let registration = || {
            gpui_kit::base::TextSelectionRegistration::new(hitbox.clone(), bounds)
                .with_text_bounds(vec![layout_bounds])
                .with_document_order(order)
        };
        window.with_global_id(self.id.clone(), |id, window| {
            window.with_element_state::<RichSelection, _>(id, |state, window| {
                let state = state.unwrap_or_else(|| {
                    let handle = gpui_kit::base::TextSelectionHandle::new(text.to_string(), cx);
                    let refresh = refresh_on_change(&handle, window, cx);
                    RichSelection {
                        handle,
                        _refresh: refresh,
                    }
                });
                state.handle.set_fallback_copy_text(text.to_string(), cx);
                *active.borrow_mut() = Some(state.handle.clone());
                if joins {
                    state.handle.register(registration(), window, cx);
                }
                ((), state)
            })
        });
        (hitbox, clip)
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        (hitbox, clip): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (hitbox, clip) = (hitbox.clone(), *clip);
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
            if phase != gpui_kit::DispatchPhase::Capture || event.button != MouseButton::Left {
                return;
            }
            // a new press replaces the click's anchor with its own: weighing
            // the selection after the next frame (below) would weigh this
            // gesture, and drop its anchor before it moves
            if SETTLING.get() == Some(window.window_handle().window_id()) {
                SETTLING.set(None);
            }
            if hitbox.is_hovered(window) {
                DRAG_CLIP.set(Some(clip));
                PRESS.set(Some(event.position));
            }
        });
        window.on_mouse_event(|event: &MouseUpEvent, phase, window, _| {
            if phase == gpui_kit::DispatchPhase::Capture
                && event.button == MouseButton::Left
                && DRAG_CLIP.take().is_some()
            {
                // the paragraphs the drag left out take part again
                window.refresh();
                SETTLING.set(Some(window.window_handle().window_id()));
            }
        });
        // A press on words that selected none (a click, still or drifting a
        // pixel or two) leaves gpui-base's selection an anchor, kept for a
        // later shift-click to extend from. While it holds one, every
        // paragraph's registration publishes to every paragraph, each frame:
        // a hover step costs the square of the window's texts. Nothing
        // extends from a click here, so once the gesture has ended one that
        // takes in no word (the click test's question, `dragged_over_words`)
        // is dropped. It is asked after the first frame past the release:
        // the release resets every paragraph's projected copy text
        // (`set_snapshot`), and until they paint again each one the
        // selection took in copies its whole text. The kit's own question,
        // `selects_text`, reads the painted runs and would answer at the
        // release, but it is private.
        //
        // The root fix is in gpui-base, and it makes this step go: its
        // `is_active` ignoring the ends a finished click left when they take
        // in no character keeps the anchor, and the shift-click from it, at
        // no cost.
        //
        // `SETTLING` holds one window: a release in another window before
        // this one paints (not one pointer's pace) leaves this anchor to its
        // next press, which drops it. A closed window's id never matches a
        // later one (gpui's ids are versioned).
        if SETTLING.get() == Some(window.window_handle().window_id()) {
            SETTLING.set(None);
            window.defer(cx, |window, cx| {
                if gpui_kit::base::TextSelection::selected_text(window, cx).is_empty() {
                    gpui_kit::base::TextSelection::clear(window, cx);
                }
            });
        }
        let run = gpui_kit::base::TextSelectionRun::new(
            self.text.clone(),
            self.layout.clone(),
            self.layout.bounds(),
        );
        if let Some(handle) = self.active_handle.borrow().as_ref() {
            let projection = handle.update_runs(&[run], cx);
            *self.selection.borrow_mut() = projection.ranges().first().cloned().flatten();
        }
        self.content.paint(window, cx);
    }
}

/// The box `range` covers on each line it touches.
fn line_boxes(layout: &TextLayout, range: &std::ops::Range<usize>) -> Vec<Bounds<Pixels>> {
    let mut boxes = Vec::new();
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return boxes;
    };
    let height = layout.line_height();
    if height <= px(0.) {
        return boxes;
    }
    let mut y = start.y;
    while y <= end.y {
        let left = if y == start.y {
            start.x
        } else {
            layout.bounds().left()
        };
        let right = if y == end.y {
            end.x
        } else {
            layout.bounds().right()
        };
        boxes.push(Bounds::from_corners(
            point(left, y),
            point(right, y + height),
        ));
        y += height;
    }
    boxes
}

pub(super) fn paint_rich_selection(
    layout: &TextLayout,
    range: &std::ops::Range<usize>,
    window: &mut Window,
    cx: &App,
) {
    let color = gpui_kit::base::Theme::global(cx).tokens.colors.selection;
    for bounds in line_boxes(layout, range) {
        window.paint_quad(fill(bounds, color));
    }
}

/// The mark on the link the arrows picked: the shell focus ring's ink,
/// 2px around the range's words, which reads at 3:1 on either theme.
fn paint_picked_link(
    layout: &TextLayout,
    range: &std::ops::Range<usize>,
    window: &mut Window,
    cx: &App,
) {
    let ring = crate::a11y::ring(crate::a11y::ink(cx));
    for bounds in line_boxes(layout, range) {
        window.paint_quad(gpui_kit::quad(
            bounds,
            px(0.),
            gpui_kit::transparent_black(),
            ring.spread_radius,
            ring.color,
            gpui_kit::BorderStyle::Solid,
        ));
    }
}

/// The layer over a paragraph that paints its selected `range` each frame,
/// and the mark on `picked`, the link the arrows are on.
fn selection_canvas(
    layout: TextLayout,
    range: std::rc::Rc<std::cell::RefCell<Option<std::ops::Range<usize>>>>,
    picked: Option<std::ops::Range<usize>>,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, cx| {
            if let Some(range) = range.borrow().as_ref() {
                paint_rich_selection(&layout, range, window, cx);
            }
            if let Some(range) = &picked {
                paint_picked_link(&layout, range, window, cx);
            }
        },
    )
    .absolute()
    .size_full()
}

impl ViewTree {
    pub(super) fn text(&mut self, node: &wire::Node) -> AnyElement {
        let wire::Node::Text(view_wire::TextNode {
            id, style, content, ..
        }) = node
        else {
            unreachable!()
        };
        let native_id = id.as_ref().map(native_id).unwrap_or_else(|| {
            let index = self.render_index;
            self.render_index += 1;
            host_id(format!("text-{index}"))
        });
        let shared: SharedString = content.clone().into();
        let styled = StyledText::new(shared.clone());
        let layout = styled.layout().clone();
        let selection_range = std::rc::Rc::new(std::cell::RefCell::new(None));
        let mut element = div().relative();
        *element.style() = Arc::unwrap_or_clone(self.style(*style));
        crate::fonts::refine_fallbacks(element.style());
        let mut element = element.id(native_id.clone());
        if !self.unselected {
            element = element.child(selection_canvas(
                layout.clone(),
                selection_range.clone(),
                None,
            ));
        }
        // The div is the Label `announce` names below; the styled text
        // itself has no AX node, so it is not a second, empty-named node
        // carrying the same content as its value.
        let element = announce(element.child(styled), accessible(node));
        #[cfg(test)]
        let element = {
            use gpui_kit::test::TestSupportExt as _;
            element.test_support()
        };
        // a control's label or a list marker is no paragraph of the selection
        // (`keeps_words_unselected`)
        if self.unselected {
            return element.into_any_element();
        }
        // A plain text is a paragraph of the selection too: a drag across
        // a label, a caption or a heading selects and copies it, as across
        // a message body.
        RichParagraph {
            id: native_id,
            content: element.into_any_element(),
            text: shared,
            layout,
            active_handle: Default::default(),
            selection: selection_range,
            order: self.selection_order.clone(),
        }
        .into_any_element()
    }

    pub(super) fn rich_text(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::RichText {
            id,
            style,
            text,
            runs,
            font_family_overrides,
            clickable_ranges,
            on_click,
            on_hover,
            tooltip,
        } = node
        else {
            unreachable!()
        };
        // an id-less paragraph is numbered, as an id-less text is, so the
        // constant `rich-text` inside never lands twice under one parent
        let native_id = id.as_ref().map(native_id).unwrap_or_else(|| {
            let index = self.render_index;
            self.render_index += 1;
            host_id(format!("rich-{index}"))
        });
        let shared: SharedString = text.clone().into();
        let mut styled = StyledText::new(shared.clone());
        styled = match runs {
            wire::RichTextRuns::Highlights(highlights) => styled.with_highlights(
                highlights
                    .iter()
                    .cloned()
                    .map(|(range, style)| (range, style.into())),
            ),
            wire::RichTextRuns::Runs(runs) => styled.with_runs(
                runs.iter()
                    .cloned()
                    .map(|mut run| {
                        run.font_family = crate::fonts::app_family(&run.font_family);
                        run.into()
                    })
                    .collect(),
            ),
        };
        styled = styled.with_font_family_overrides(
            font_family_overrides
                .iter()
                .map(|(range, family)| (range.clone(), crate::fonts::app_family(family))),
        );
        let layout = styled.layout().clone();
        let rich_id: ElementId = "rich-text".into();
        let mut interactive = InteractiveText::new(rich_id, styled);
        // a text with clickable ranges is one Tab stop whose arrows pick a
        // link, and each range a Link to assistive technology; a pointer
        // click on a range, Enter on the picked link and a press from
        // assistive technology all reach the view as the range's click
        let linked = match on_click {
            Some(handler) if !clickable_ranges.is_empty() => {
                let (element, linking) =
                    self.linked_box(&native_id, text, clickable_ranges, *handler, window, cx);
                let press = linking.press.clone();
                // a click that ended a drag selected words; it pressed nothing
                interactive =
                    interactive.on_click(clickable_ranges.clone(), move |index, window, cx| {
                        let up = window.mouse_position();
                        let down = PRESS.get().unwrap_or(up);
                        if !dragged_over_words(down, up, window, cx) {
                            press(index, cx)
                        }
                    });
                Some((element, linking))
            }
            _ => None,
        };
        let hover_handler = *on_hover;
        let tooltip_request = *tooltip;
        if hover_handler.is_some() || tooltip_request.is_some() {
            let view = cx.entity().downgrade();
            interactive = interactive.on_hover(move |index, event, _, cx| {
                let character_index = index.and_then(|index| u32::try_from(index).ok());
                let _ = view.update(cx, |_, cx| {
                    if let Some(handler) = hover_handler {
                        cx.emit(wire::Event::RichTextHover {
                            handler,
                            event: wire::RichTextHover {
                                index: character_index,
                                position: event.position,
                                pressed_button: event.pressed_button.map(Into::into),
                                modifiers: event.modifiers,
                            },
                        });
                    }
                    if let (Some(request), Some(character_index)) =
                        (tooltip_request, character_index)
                    {
                        cx.emit(wire::Event::TooltipRequest {
                            request,
                            character_index: Some(character_index),
                        });
                    }
                });
            });
        }
        if let Some(request) = *tooltip {
            let parent = cx.entity().downgrade();
            interactive = interactive.tooltip(move |index, _, cx| {
                let character_index = u32::try_from(index).ok()?;
                let content = parent
                    .update(cx, |this, cx| {
                        let content =
                            rich_tooltip_content(&this.tooltips, request, character_index);
                        cx.emit(wire::Event::TooltipRequest {
                            request,
                            character_index: Some(character_index),
                        });
                        content
                    })
                    .ok()
                    .flatten();
                content
                    .map(|content| super::tooltip_containment::build(parent.clone(), content, cx))
            });
        }
        let picked = linked
            .as_ref()
            .and_then(|(_, linking)| linking.picked)
            .map(|index| clickable_ranges[index].clone());
        let selection_range = std::rc::Rc::new(std::cell::RefCell::new(None));
        let selection = selection_canvas(layout.clone(), selection_range.clone(), picked);
        let content = match linked {
            Some((mut boxed, linking)) => {
                *boxed.style() = Arc::unwrap_or_clone(self.style(*style));
                let interactive = links::Linked::new(interactive, linking, cx.entity().downgrade());
                boxed.child(selection).child(interactive).into_any_element()
            }
            None => {
                let mut content = div()
                    .id(native_id.clone())
                    .relative()
                    .child(selection)
                    .child(interactive);
                *content.style() = Arc::unwrap_or_clone(self.style(*style));
                content.into_any_element()
            }
        };
        // a control's label or a list marker is no paragraph of the selection
        // (`keeps_words_unselected`)
        if self.unselected {
            return content;
        }
        RichParagraph {
            id: native_id,
            content,
            text: shared,
            layout,
            active_handle: Default::default(),
            selection: selection_range,
            order: self.selection_order.clone(),
        }
        .into_any_element()
    }
}
