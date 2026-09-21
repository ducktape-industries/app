use super::*;

pub(super) struct RichSelection {
    pub(super) handle: gpui_kit::base::TextSelectionHandle,
    pub(super) _refresh: Subscription,
}

// Layout and hit-testing stay native. One participant receives every span's
// measured glyph run so copying concatenates source text, never visual padding.
pub(super) struct RichParagraph {
    pub(super) id: ElementId,
    pub(super) content: AnyElement,
    pub(super) layouts: Vec<(SharedString, TextLayout)>,
    pub(super) handle: gpui_kit::base::TextSelectionHandle,
    pub(super) selections: std::rc::Rc<std::cell::RefCell<Vec<Option<std::ops::Range<usize>>>>>,
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
        Some(self.id.clone())
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
    ) {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        // Register the containing hitbox before link hitboxes, so selectable
        // paragraph geometry cannot cover its own interactive spans.
        self.content.prepaint(window, cx);
        self.handle.register(
            gpui_kit::base::TextSelectionRegistration::new(hitbox, bounds).with_text_bounds(
                self.layouts
                    .iter()
                    .map(|(_, layout)| layout.bounds())
                    .collect(),
            ),
            window,
            cx,
        );
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
        let runs = self
            .layouts
            .iter()
            .enumerate()
            .map(|(index, (text, layout))| {
                gpui_kit::base::TextSelectionRun::new(text.clone(), layout.clone(), layout.bounds())
                    .with_document_order(index as u64)
            })
            .collect::<Vec<_>>();
        let projection = self.handle.update_runs(&runs, cx);
        *self.selections.borrow_mut() = projection.ranges().to_vec();
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
        let wire::Node::Text {
            key,
            content,
            size,
            color,
            width,
            font,
            align_x,
            options,
            heading,
            ..
        } = node
        else {
            unreachable!()
        };
        let mut element = text_options(
            dimensions(div().min_w_0().max_w_full(), *width, options.height),
            *font,
            *align_x,
            options,
        )
        // a Label: assistive technology reads its content as its name
        .child(gpui_kit::Text::new(
            key.clone().into(),
            content.clone().into(),
        ));
        let intrinsic_label = options.wrapping == Some(wire::Wrapping::None)
            && matches!(width, None | Some(wire::Length::Shrink));
        if intrinsic_label {
            // Shrink-sized labels keep their natural width; a Fill
            // sibling takes the remaining space, not their letters.
            element = element.flex_shrink_0();
        }
        if let Some(size) = size {
            element = element.text_size(px(*size));
        }
        if let Some(color) = color {
            element = element.text_color(rgba(*color));
        }
        #[cfg(test)]
        {
            element = element.relative().child(self.measure(key, cx));
        }
        #[cfg(not(test))]
        let _ = (key, cx);
        if heading.is_some() {
            return announce(element.id(format!("{key}/heading")), accessible(node))
                .into_any_element();
        }
        element.into_any_element()
    }

    pub(super) fn rich_text(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::RichText {
            key,
            spans,
            size,
            color,
            font,
            width,
            align_x,
            options,
            on_link,
        } = node
        else {
            unreachable!()
        };
        let canonical = spans
            .iter()
            .map(|span| span.content.as_str())
            .collect::<String>();
        let selection = self.rich_selections.entry(key.clone()).or_insert_with(|| {
            let handle = gpui_kit::base::TextSelectionHandle::new(canonical.clone(), cx);
            let refresh = handle.refresh_window_on_change(window, cx);
            RichSelection {
                handle,
                _refresh: refresh,
            }
        });
        selection.handle.set_fallback_copy_text(canonical, cx);
        let handle = selection.handle.clone();
        let mut content = text_options(
            dimensions(div().flex().flex_col(), *width, options.height),
            *font,
            *align_x,
            options,
        );
        if let Some(size) = size {
            content = content.text_size(px(*size));
        }
        if let Some(color) = color {
            content = content.text_color(rgba(*color));
        }
        let base_size = size
            .map(px)
            .unwrap_or_else(|| window.text_style().font_size.to_pixels(window.rem_size()));
        let line_height = match options.line_height {
            Some(wire::LineHeight::Absolute(height)) => px(height),
            Some(wire::LineHeight::Relative(height)) => base_size * height,
            None => window
                .text_style()
                .line_height
                .to_pixels(base_size.into(), window.rem_size()),
        };
        let native_row = || {
            let mut row = div().flex().flex_row().items_baseline().min_h(line_height);
            if options.wrapping != Some(wire::Wrapping::None) {
                row = row.flex_wrap();
            }
            horizontal_align(row, *align_x)
        };
        let mut row = native_row();
        let mut layouts = Vec::new();
        let selections = std::rc::Rc::new(std::cell::RefCell::new(Vec::<
            Option<std::ops::Range<usize>>,
        >::new()));
        // ONE SPAN IS ONE PARAGRAPH, laid out by the text system: it wraps
        // where a plain label would, with the same spacing, and the whole
        // run is one selection layout. Only a paragraph of SEVERAL spans is
        // cut into word fragments, so native flex can wrap across the style
        // changes — the cut is what made a long plain message read
        // differently from every other text on screen.
        let whole = spans.len() == 1;
        for (span_index, span) in spans.iter().enumerate() {
            // Native flex wraps at Unicode word boundaries; padding is paint
            // geometry only, while every copied fragment retains source bytes.
            let fragments = if whole {
                vec![span.content.as_str()]
            } else {
                span.content.split_word_bounds().collect::<Vec<_>>()
            };
            for (index, fragment) in fragments.iter().enumerate() {
                let mut run_options = options.clone();
                run_options.font = span.font.clone().or_else(|| options.font.clone());
                run_options.line_height = span.line_height.or(options.line_height);
                let plate = if whole {
                    div().min_w_0().w_full()
                } else {
                    div().flex_shrink_0().max_w_full()
                };
                let mut paint = text_options(plate, *font, None, &run_options);
                if let Some(size) = span.size {
                    paint = paint.text_size(px(size));
                }
                if let Some(color) = span.color {
                    paint = paint.text_color(rgba(color));
                }
                let mut padding = span.padding.unwrap_or_default();
                if index > 0 {
                    padding.left = 0.;
                }
                if index + 1 < fragments.len() {
                    padding.right = 0.;
                }
                paint = decoration(pad(paint, Some(padding)), span.background, span.border);
                let mut style = HighlightStyle::default();
                if span.underline {
                    style.underline = Some(UnderlineStyle {
                        thickness: px(1.),
                        color: span.color.map(rgba),
                        wavy: false,
                    });
                }
                if span.strikethrough {
                    style.strikethrough = Some(StrikethroughStyle {
                        thickness: px(1.),
                        color: span.color.map(rgba),
                    });
                }
                let text: SharedString = (*fragment).to_owned().into();
                let styled =
                    StyledText::new(text.clone()).with_highlights([(0..text.len(), style)]);
                let layout = styled.layout().clone();
                let run_index = layouts.len();
                layouts.push((text.clone(), layout.clone()));
                let ranges = selections.clone();
                let selection = canvas(
                    |_, _, _| (),
                    move |_, (), window, cx| {
                        if let Some(Some(range)) = ranges.borrow().get(run_index) {
                            paint_rich_selection(&layout, range, window, cx);
                        }
                    },
                )
                .absolute()
                .size_full();
                let id = format!("{key}-span-{span_index}-{index}");
                let mut painted = paint.id(id).child(selection).child(styled);
                if let (Some(handler), Some(link)) = (on_link, &span.link) {
                    let handler = *handler;
                    let link = link.clone();
                    painted = crate::a11y::keyboard(
                        painted.role(gpui_kit::Role::Link).aria_label(text.clone()),
                    );
                    painted =
                        painted
                            .cursor_pointer()
                            .on_click(cx.listener(move |_, _, window, cx| {
                                if gpui_kit::base::TextSelection::has_selection(window, cx) {
                                    return;
                                }
                                cx.emit(wire::Event::Input {
                                    handler,
                                    text: link.clone(),
                                });
                            }));
                }
                // a whole paragraph keeps its newlines: the text system
                // breaks the lines, and the selection layout spans them
                let newline = !whole && fragment.contains('\n');
                if newline {
                    // Explicit source line breaks remain real measured text, not
                    // injected spaces in the selection/copy representation.
                    // One native row per explicit source line preserves empty
                    // lines too. The newline participates in copy, not sizing.
                    painted = painted.w(px(0.)).h(px(0.));
                    row = row.child(painted);
                    content = content.child(row);
                    row = native_row();
                    continue;
                }
                row = row.child(painted);
            }
        }
        content = content.child(row);
        RichParagraph {
            id: key.clone().into(),
            content: content.into_any_element(),
            layouts,
            handle,
            selections,
        }
        .into_any_element()
    }
}
