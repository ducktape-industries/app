use super::*;
use crate::render::native_id;

fn rich_tooltip_content(
    node: &wire::Node,
    request: u32,
    character_index: u32,
) -> Option<wire::Node> {
    if let wire::Node::RichText {
        tooltip: Some(tooltip),
        ..
    } = node
        && tooltip.request == request
        && tooltip.character_index == Some(character_index)
    {
        return tooltip.content.as_deref().cloned();
    }
    node.children()
        .iter()
        .find_map(|child| rich_tooltip_content(child, request, character_index))
}

pub(super) struct RichSelection {
    pub(super) handle: gpui_kit::base::TextSelectionHandle,
    pub(super) _refresh: Subscription,
}

// Layout and hit-testing stay native. One participant receives every span's
// measured glyph run so copying concatenates source text, never visual padding.
pub(super) struct RichParagraph {
    pub(super) id: Option<ElementId>,
    pub(super) content: AnyElement,
    pub(super) text: SharedString,
    pub(super) layout: TextLayout,
    pub(super) fallback: RichSelection,
    pub(super) active_handle:
        std::rc::Rc<std::cell::RefCell<Option<gpui_kit::base::TextSelectionHandle>>>,
    pub(super) selection: std::rc::Rc<std::cell::RefCell<Option<std::ops::Range<usize>>>>,
}

impl IntoElement for RichParagraph {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for RichParagraph {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        self.id.clone()
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
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        // Register the containing hitbox before link hitboxes, so selectable
        // paragraph geometry cannot cover its own interactive spans.
        self.content.prepaint(window, cx);
        let text = self.text.clone();
        let layout_bounds = self.layout.bounds();
        let active = self.active_handle.clone();
        window.with_optional_element_state::<RichSelection, _>(id, |state, window| {
            let state = match state {
                Some(state) => state.unwrap_or_else(|| {
                    let handle = gpui_kit::base::TextSelectionHandle::new(text.to_string(), cx);
                    let refresh = handle.refresh_window_on_change(window, cx);
                    RichSelection {
                        handle,
                        _refresh: refresh,
                    }
                }),
                None => {
                    *active.borrow_mut() = Some(self.fallback.handle.clone());
                    self.fallback.handle.register(
                        gpui_kit::base::TextSelectionRegistration::new(hitbox.clone(), bounds)
                            .with_text_bounds(vec![layout_bounds]),
                        window,
                        cx,
                    );
                    return ((), None);
                }
            };
            state.handle.set_fallback_copy_text(text.to_string(), cx);
            *active.borrow_mut() = Some(state.handle.clone());
            state.handle.register(
                gpui_kit::base::TextSelectionRegistration::new(hitbox.clone(), bounds)
                    .with_text_bounds(vec![layout_bounds]),
                window,
                cx,
            );
            ((), Some(state))
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

pub(super) fn paint_rich_selection(
    layout: &TextLayout,
    range: &std::ops::Range<usize>,
    window: &mut Window,
    cx: &App,
) {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return;
    };
    let height = layout.line_height();
    if height <= px(0.) {
        return;
    }
    let color = gpui_kit::base::Theme::global(cx).tokens.colors.selection;
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
        window.paint_quad(fill(
            Bounds::from_corners(point(left, y), point(right, y + height)),
            color,
        ));
        y += height;
    }
}

impl ViewTree {
    pub(super) fn text(&mut self, node: &wire::Node, cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Text(view_wire::TextNode {
            id, style, content, ..
        }) = node
        else {
            unreachable!()
        };
        let native_id = id.as_ref().map(native_id).unwrap_or_else(|| {
            let index = self.render_index;
            self.render_index += 1;
            ElementId::NamedInteger("guest-text".into(), index)
        });
        let mut element = div();
        *element.style() = style.clone();
        crate::shell::refine_fallbacks(element.style());
        let text_id = native_id.clone();
        let element = element
            .id(native_id)
            // A Label: assistive technology reads its content as its name.
            .child(gpui_kit::Text::new(text_id, content.clone().into()));
        #[cfg(test)]
        let element = if id.is_some() {
            element.child(self.measure(&self.authored_path, cx))
        } else {
            element
        };
        #[cfg(not(test))]
        let _ = cx;
        announce(element, accessible(node)).into_any_element()
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
        let native_id = id.as_ref().map(native_id);
        let shared: SharedString = text.clone().into();
        let mut styled = StyledText::new(shared.clone());
        styled = match runs {
            wire::RichTextRuns::Highlights(highlights) => styled.with_highlights(
                highlights
                    .iter()
                    .cloned()
                    .map(|(range, style)| (range, style.into())),
            ),
            wire::RichTextRuns::Runs(runs) => {
                styled.with_runs(runs.iter().cloned().map(Into::into).collect())
            }
        };
        styled = styled.with_font_family_overrides(font_family_overrides.iter().cloned());
        let layout = styled.layout().clone();
        let rich_id: ElementId = "rich-text".into();
        let mut interactive = InteractiveText::new(rich_id, styled);
        if let Some(handler) = on_click {
            let handler = *handler;
            let view = cx.entity().downgrade();
            interactive =
                interactive.on_click(clickable_ranges.clone(), move |index, window, cx| {
                    if !gpui_kit::base::TextSelection::has_selection(window, cx) {
                        let _ = view.update(cx, |this, cx| {
                            this.user_activation.set(Some(handler));
                            cx.emit(wire::Event::Select {
                                handler,
                                index: index as u32,
                            });
                        });
                    }
                });
        }
        let hover_handler = *on_hover;
        let tooltip_request = tooltip.as_ref().map(|tooltip| tooltip.request);
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
        if let Some(tooltip) = tooltip {
            let request = tooltip.request;
            let parent = cx.entity().downgrade();
            interactive = interactive.tooltip(move |index, _, cx| {
                let character_index = u32::try_from(index).ok()?;
                let content = parent
                    .update(cx, |this, cx| {
                        let content = rich_tooltip_content(&this.root, request, character_index);
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
        let selection_range = std::rc::Rc::new(std::cell::RefCell::new(None));
        let selection_for_paint = selection_range.clone();
        let selection_layout = layout.clone();
        let selection = canvas(
            |_, _, _| (),
            move |_, (), window, cx| {
                if let Some(range) = selection_for_paint.borrow().as_ref() {
                    paint_rich_selection(&selection_layout, range, window, cx);
                }
            },
        )
        .absolute()
        .size_full();
        let mut content = div().relative().child(selection).child(interactive);
        *content.style() = style.clone();
        let handle = gpui_kit::base::TextSelectionHandle::new(text.clone(), cx);
        let refresh = handle.refresh_window_on_change(window, cx);
        RichParagraph {
            id: native_id,
            content: content.into_any_element(),
            text: shared,
            layout,
            fallback: RichSelection {
                handle,
                _refresh: refresh,
            },
            active_handle: Default::default(),
            selection: selection_range,
        }
        .into_any_element()
    }
}
