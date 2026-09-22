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
            id,
            style,
            interactivity,
            children,
        } = node
        else {
            unreachable!()
        };
        let mut element = div();
        *element.style() = style.clone();
        let native_id = id
            .as_ref()
            .or(interactivity.id.as_ref())
            .and_then(|id| id.to_gpui().ok())
            .unwrap_or_else(|| {
                let index = self.render_index;
                self.render_index += 1;
                ElementId::NamedInteger("guest-container".into(), index)
            });
        let mut element = element.id(native_id);
        if let Some(group) = &interactivity.group {
            element = element.group(group.clone());
        }
        if let Some(style) = &interactivity.hover {
            let style = style.clone();
            element = element.hover(move |_| style);
        }
        if let Some(style) = &interactivity.active {
            let style = style.clone();
            element = element.active(move |_| style);
        }
        if let Some(group) = &interactivity.group_hover {
            let style = group.style.clone();
            element = element.group_hover(group.group.clone(), move |_| style);
        }
        if let Some(group) = &interactivity.group_active {
            let style = group.style.clone();
            element = element.group_active(group.group.clone(), move |_| style);
        }
        if let Some(handler) = interactivity.on_click {
            element = element
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(wire::Event::Message(handler))));
        }
        if let Some(key) = node.key() {
            let kind = std::mem::discriminant(node);
            let restore = self
                .presentation
                .focused_container
                .as_ref()
                .is_some_and(|(saved, saved_kind)| saved == key && *saved_kind == kind);
            if restore {
                self.presentation.focused_container = None;
                let (_, handle) = self
                    .focus_targets
                    .entry(key.to_owned())
                    .or_insert_with(|| (kind, cx.focus_handle()));
                handle.focus(window, cx);
            }
            if let Some((_, handle)) = self.focus_targets.get(key) {
                element = element.track_focus(handle);
            }
            element = element.child(self.measure(key, cx));
        }
        for child in children {
            element = element.child(self.node(child, window, cx));
        }
        #[cfg(test)]
        let element = {
            use gpui_kit::test::TestSupportExt as _;
            element.test_support()
        };
        element.into_any_element()
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
}
