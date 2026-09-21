use super::*;

impl ViewTree {
    pub(super) fn linear(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Linear {
            axis,
            spacing,
            padding,
            width,
            height,
            background,
            border,
            children,
            align,
            max_width,
            clip,
            wrap,
            ..
        } = node
        else {
            unreachable!()
        };
        let mut element = dimensions(div().flex(), *width, *height);
        element = match axis {
            wire::Axis::Column => element.flex_col(),
            wire::Axis::Row => element.flex_row(),
        };
        if let Some(gap) = spacing {
            element = element.gap(px(*gap));
        }
        if let Some(width) = max_width {
            element = element.max_w(px(*width));
        }
        if *clip {
            element = element.overflow_hidden();
        }
        if wrap.is_some() {
            element = element.flex_wrap();
        }
        element = cross_align(element, *align);
        element = decoration(pad(element, *padding), *background, *border);
        for child in children {
            element = element.child(self.node(child, window, cx));
        }
        self.focusable_container(node, element, window, cx)
    }

    pub(super) fn keyed_column(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::KeyedColumn {
            key,
            keys,
            spacing,
            padding,
            width,
            height,
            background,
            border,
            children,
            align,
            max_width,
            ..
        } = node
        else {
            unreachable!()
        };
        let mut element = decoration(
            pad(
                dimensions(div().flex().flex_col(), *width, *height),
                *padding,
            ),
            *background,
            *border,
        );
        if let Some(gap) = spacing {
            element = element.gap(px(*gap));
        }
        element = cross_align(element, *align);
        if let Some(width) = max_width {
            element = element.max_w(px(*width));
        }
        for (index, child) in children.iter().enumerate() {
            let content = self.node(child, window, cx);
            let identity = keys.as_ref().and_then(|keys| keys.get(index));
            element = match identity {
                Some(identity) => {
                    let row = format!("{key}/@row:{}", identity.virtual_key());
                    element.child(
                        div()
                            .relative()
                            .child(content)
                            .child(self.measure(&row, cx)),
                    )
                }
                None => element.child(content),
            };
        }
        element.into_any_element()
    }

    pub(super) fn container(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Container {
            key,
            content,
            width,
            height,
            padding,
            border,
            background,
            max_width,
            max_height,
            clip,
            align_x,
            align_y,
            shadow,
            ..
        } = node
        else {
            unreachable!()
        };
        let color = match background {
            Some(wire::Background::Color(color)) => Some(*color),
            _ => None,
        };
        let mut element = shadows(
            decoration(
                pad(
                    dimensions(div().relative().flex(), *width, *height),
                    *padding,
                ),
                color,
                *border,
            ),
            *shadow,
        );
        element = horizontal_align(element, *align_x);
        element = vertical_align(element, *align_y);
        if let Some(width) = max_width {
            element = element.max_w(px(*width));
        }
        if let Some(height) = max_height {
            element = element.max_h(px(*height));
        }
        if *clip {
            element = element.overflow_hidden();
        }
        let element = element
            .child(self.node(content, window, cx))
            .child(self.measure(key, cx));
        self.focusable_container(node, element, window, cx)
    }

    pub(super) fn responsive(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Responsive {
            key,
            content,
            width,
            height,
        } = node
        else {
            unreachable!()
        };
        let weak = cx.entity().downgrade();
        let key = key.clone();
        let measure = canvas(
            move |bounds, _, cx| {
                let size = [
                    f32::from(bounds.size.width) as f64,
                    f32::from(bounds.size.height) as f64,
                ];
                let _ = weak.update(cx, |this, cx| {
                    let changed = this.containers.get(&key) != Some(&size);
                    if changed {
                        this.containers.insert(key, size);
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        dimensions(div().relative(), *width, *height)
            .child(self.node(content, window, cx))
            .child(measure)
            .into_any_element()
    }

    pub(super) fn when(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::When {
            condition,
            children,
            ..
        } = node
        else {
            unreachable!()
        };
        let mut element = div().flex().flex_col();
        if condition.matches(&self.containers) {
            for child in children {
                element = element.child(self.node(child, window, cx));
            }
        }
        element.into_any_element()
    }

    pub(super) fn grid(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Grid {
            key,
            children,
            width,
            height,
            padding,
            spacing,
            columns,
            fluid,
            aspect,
            background,
            border,
        } = node
        else {
            unreachable!()
        };
        let available = self
            .bounds
            .get(key)
            .map_or(f32::from(window.viewport_size().width), |bounds| {
                f32::from(bounds.size.width)
            });
        let columns = fluid
            .filter(|value| *value > 0.0)
            .map_or(columns.unwrap_or(1), |value| {
                (available / value).ceil().max(1.0) as u32
            })
            .max(1);
        let mut grid = decoration(
            pad(
                dimensions(div().flex().flex_wrap(), *width, *height),
                *padding,
            ),
            *background,
            *border,
        );
        let gap = spacing.unwrap_or_default();
        grid = grid.gap(px(gap));
        let cell_width =
            ((available - gap * columns.saturating_sub(1) as f32) / columns as f32).max(0.0);
        for child in children {
            let cell = div()
                .w(px(cell_width))
                .h(px(cell_width / aspect.unwrap_or(1.0).max(0.001)));
            grid = grid.child(cell.child(self.node(child, window, cx)));
        }
        grid.relative()
            .child(self.measure(key, cx))
            .into_any_element()
    }

    pub(super) fn stack(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Stack {
            children,
            width,
            height,
            padding,
            background,
            border,
            clip,
            under,
            ..
        } = node
        else {
            unreachable!()
        };
        let mut element = decoration(
            pad(dimensions(div().relative(), *width, *height), *padding),
            *background,
            *border,
        );
        if *clip {
            element = element.overflow_hidden();
        }
        if *under == 0 {
            element = element.grid().grid_cols(1).grid_rows(1);
        }
        for (index, child) in children.iter().enumerate() {
            let content = self.node(child, window, cx);
            element = match (*under, index) {
                (0, _) => element.child(div().col_start(1).row_start(1).child(content)),
                (base, index) if index == base as usize => element.child(content),
                _ => element.child(div().absolute().inset_0().child(content)),
            };
        }
        element.into_any_element()
    }

    pub(super) fn pin(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Pin {
            content,
            x,
            y,
            width,
            height,
            ..
        } = node
        else {
            unreachable!()
        };
        dimensions(div().absolute().left(px(*x)).top(px(*y)), *width, *height)
            .child(self.node(content, window, cx))
            .into_any_element()
    }

    pub(super) fn measure(&self, key: &str, cx: &Context<Self>) -> impl IntoElement + use<> {
        let route = key.to_owned();
        let weak = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    let changed = this.bounds.get(&route) != Some(&bounds);
                    if changed {
                        this.bounds.insert(route, bounds);
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    pub(super) fn flex(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Flex {
            layout,
            children,
            items,
            background,
            border,
            ..
        } = node
        else {
            unreachable!()
        };
        let mut element = decoration(
            pad(
                dimensions(div().flex(), layout.width, layout.height),
                layout.padding,
            ),
            *background,
            *border,
        );
        element = match layout.direction {
            wire::FlexDirection::Row => element.flex_row(),
            wire::FlexDirection::RowReverse => element.flex_row_reverse(),
            wire::FlexDirection::Column => element.flex_col(),
            wire::FlexDirection::ColumnReverse => element.flex_col_reverse(),
        };
        element = match layout.wrap {
            wire::FlexWrap::NoWrap => element,
            wire::FlexWrap::Wrap => element.flex_wrap(),
            wire::FlexWrap::WrapReverse => element.flex_wrap_reverse(),
        };
        if let Some(width) = layout.max_width {
            element = element.max_w(px(width));
        }
        if let Some(height) = layout.max_height {
            element = element.max_h(px(height));
        }
        if let Some(gap) = layout.row_gap {
            element = element.gap_y(px(gap));
        }
        if let Some(gap) = layout.column_gap {
            element = element.gap_x(px(gap));
        }
        if layout.clip {
            element = element.overflow_hidden();
        }
        if let Some(alignment) = layout.justify {
            element = justify(element, alignment);
        }
        if let Some(alignment) = layout.items {
            element = align_items(element, alignment);
        }
        if let Some(alignment) = layout.content {
            element = match alignment {
                wire::FlexContentAlignment::Start | wire::FlexContentAlignment::FlexStart => {
                    element.content_start()
                }
                wire::FlexContentAlignment::End | wire::FlexContentAlignment::FlexEnd => {
                    element.content_end()
                }
                wire::FlexContentAlignment::Center => element.content_center(),
                wire::FlexContentAlignment::SpaceBetween => element.content_between(),
                wire::FlexContentAlignment::SpaceAround => element.content_around(),
                wire::FlexContentAlignment::SpaceEvenly => element.content_evenly(),
                wire::FlexContentAlignment::Stretch => element.content_stretch(),
            };
        }
        let mut order: Vec<_> = children.iter().enumerate().collect();
        order.sort_by_key(|(index, _)| items.get(*index).map_or(0, |item| item.order));
        for (index, child) in order {
            let mut item = div();
            if let Some(rules) = items.get(index) {
                item.style().flex_grow = rules.grow;
                item.style().flex_shrink = Some(rules.shrink);
                item.style().flex_basis = match rules.basis {
                    wire::FlexBasis::Auto | wire::FlexBasis::Content => Some(auto()),
                    wire::FlexBasis::Fixed(value) => Some(px(value).into()),
                    wire::FlexBasis::Percent(value) => Some(relative(value / 100.0).into()),
                };
                if let Some(alignment) = rules.align {
                    item = match alignment {
                        wire::FlexItemAlignment::Start => item.self_start(),
                        wire::FlexItemAlignment::FlexStart => item.self_flex_start(),
                        wire::FlexItemAlignment::End => item.self_end(),
                        wire::FlexItemAlignment::FlexEnd => item.self_flex_end(),
                        wire::FlexItemAlignment::Center => item.self_center(),
                        wire::FlexItemAlignment::Baseline => item.self_baseline(),
                        wire::FlexItemAlignment::Stretch => item.self_stretch(),
                    };
                }
                let margin = |value| match value {
                    wire::FlexMargin::Zero => px(0.0).into(),
                    wire::FlexMargin::Auto => auto(),
                    wire::FlexMargin::Fixed(value) => px(value).into(),
                    wire::FlexMargin::Percent(value) => relative(value / 100.0).into(),
                };
                item = item
                    .mt(margin(rules.margins.top))
                    .mr(margin(rules.margins.right))
                    .mb(margin(rules.margins.bottom))
                    .ml(margin(rules.margins.left));
            }
            element = element.child(item.child(self.node(child, window, cx)));
        }
        let mut outer = dimensions(div(), layout.surface_width, layout.surface_height);
        if let Some(width) = layout.surface_max_width {
            outer = outer.max_w(px(width));
        }
        outer.child(element).into_any_element()
    }
}
