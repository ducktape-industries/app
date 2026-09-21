use super::*;

pub(super) fn native_cursor(cursor: Option<wire::mouse::Cursor>) -> CursorStyle {
    use wire::mouse::Cursor as C;
    match cursor {
        Some(C::ResizingHorizontally) => CursorStyle::ResizeLeftRight,
        Some(C::ResizingVertically) => CursorStyle::ResizeUpDown,
        Some(C::ResizingDiagonallyUp) => CursorStyle::ResizeUpRightDownLeft,
        Some(C::ResizingDiagonallyDown) => CursorStyle::ResizeUpLeftDownRight,
        Some(C::ResizingColumn) => CursorStyle::ResizeColumn,
        Some(C::ResizingRow) => CursorStyle::ResizeRow,
        Some(C::Pointer) => CursorStyle::PointingHand,
        Some(C::Grab) => CursorStyle::OpenHand,
        Some(C::Grabbing | C::Move | C::AllScroll) => CursorStyle::ClosedHand,
        Some(C::Text) => CursorStyle::IBeam,
        Some(C::Cell | C::Crosshair) => CursorStyle::Crosshair,
        Some(C::NoDrop | C::NotAllowed) => CursorStyle::OperationNotAllowed,
        Some(C::Alias) => CursorStyle::DragLink,
        Some(C::Copy) => CursorStyle::DragCopy,
        Some(C::ContextMenu) => CursorStyle::ContextualMenu,
        Some(
            C::None
            | C::Hidden
            | C::Idle
            | C::Help
            | C::Progress
            | C::Wait
            | C::ZoomIn
            | C::ZoomOut,
        )
        | None => CursorStyle::Arrow,
    }
}

pub(super) fn rgba(color: wire::Rgba) -> Hsla {
    let [r, g, b, a] = color.0;
    gpui_kit::Rgba { r, g, b, a }.into()
}

pub(super) fn named_overlay(label: &Option<String>, children: &[wire::Node]) -> bool {
    label.as_deref().is_some_and(|label| !label.is_empty()) && children.len() > 1
}

pub(super) fn has_named_overlay(node: &wire::Node) -> bool {
    match node {
        wire::Node::Overlay {
            label, children, ..
        } if named_overlay(label, children) => true,
        _ => node.children().iter().any(has_named_overlay),
    }
}

pub(super) fn content_dimensions(
    node: &wire::Node,
) -> (Option<wire::Length>, Option<wire::Length>) {
    match node {
        wire::Node::Space { width, height }
        | wire::Node::Linear { width, height, .. }
        | wire::Node::KeyedColumn { width, height, .. }
        | wire::Node::Grid { width, height, .. }
        | wire::Node::Container { width, height, .. }
        | wire::Node::Hover { width, height, .. }
        | wire::Node::Scroll { width, height, .. }
        | wire::Node::Stack { width, height, .. }
        | wire::Node::Responsive { width, height, .. } => (*width, *height),
        wire::Node::MouseArea { content, .. } => content_dimensions(content),
        // an overlay always renders full-size (see its arm)
        wire::Node::Overlay { .. } => (Some(wire::Length::Fill), Some(wire::Length::Fill)),
        // layout-transparent wrappers take their content's size: a sensor
        // around a press area around a Fill row is still a Fill row. Under a
        // cached guest mount nothing stretches an auto-sized wrapper, so a
        // wrapper that stops here leaves every Fill below it content-tall.
        wire::Node::ResizeHandle { content, .. } | wire::Node::Lazy { content, .. } => {
            content_dimensions(content)
        }
        wire::Node::Sensor { child, .. } => content_dimensions(child),
        _ => (None, None),
    }
}

pub(super) fn dimensions<T: Styled>(
    mut element: T,
    width: Option<wire::Length>,
    height: Option<wire::Length>,
) -> T {
    element = match width {
        Some(wire::Length::Fixed(value)) => element.w(px(value)).min_w(px(value)),
        Some(wire::Length::Fill) => element.w_full().min_w_0(),
        Some(wire::Length::FillPortion(_)) => element.flex_1(),
        Some(wire::Length::Shrink) | None => element,
    };
    match height {
        Some(wire::Length::Fixed(value)) => element.h(px(value)).min_h(px(value)),
        Some(wire::Length::Fill) => element.h_full().min_h_0(),
        Some(wire::Length::FillPortion(_)) => element.flex_1(),
        // Shrink is "as tall as what is in you", and leaving the element alone
        // does not say that: a flex child stretches to its row's cross axis by
        // default, which is the parent's height every time and the content's
        // never. Refusing the stretch is the other half of asking to shrink.
        // An author who leaves the height out (`None`) is not asking for
        // anything and keeps the stretch.
        Some(wire::Length::Shrink) => element.self_start(),
        None => element,
    }
}

pub(super) fn pad<T: Styled>(element: T, padding: Option<wire::Edges>) -> T {
    match padding {
        Some(edges) => element
            .pt(px(edges.top))
            .pr(px(edges.right))
            .pb(px(edges.bottom))
            .pl(px(edges.left)),
        None => element,
    }
}

pub(super) fn decoration<T: Styled>(
    mut element: T,
    background: Option<wire::Rgba>,
    border: Option<wire::Border>,
) -> T {
    if let Some(color) = background {
        element = element.bg(rgba(color));
    }
    if let Some(border) = border {
        if let Some(color) = border.color {
            element = element.border_color(rgba(color));
        }
        if let Some(width) = border.width {
            element = element.border(px(width));
        }
        if let Some([tl, tr, br, bl]) = border.radius {
            element = element
                .rounded_tl(px(tl))
                .rounded_tr(px(tr))
                .rounded_br(px(br))
                .rounded_bl(px(bl));
        }
    }
    element
}

pub(super) fn button_style(button: Button, preset: wire::ButtonPreset) -> Button {
    match preset {
        wire::ButtonPreset::Primary => button.primary(),
        // A secondary action is an outlined button: the filled grey block
        // is reserved for `Background` (a tab, a chip).
        wire::ButtonPreset::Secondary => button.outline(),
        wire::ButtonPreset::Success => button.success(),
        wire::ButtonPreset::Warning => button.warning(),
        wire::ButtonPreset::Danger => button.danger(),
        wire::ButtonPreset::Text => button.link(),
        wire::ButtonPreset::Background => button.secondary(),
        wire::ButtonPreset::Subtle => button.ghost(),
    }
}

pub(super) fn font_weight(weight: wire::Weight) -> FontWeight {
    match weight {
        wire::Weight::Thin => FontWeight::THIN,
        wire::Weight::ExtraLight => FontWeight::EXTRA_LIGHT,
        wire::Weight::Light => FontWeight::LIGHT,
        wire::Weight::Normal => FontWeight::NORMAL,
        wire::Weight::Medium => FontWeight::MEDIUM,
        wire::Weight::Semibold => FontWeight::SEMIBOLD,
        wire::Weight::Bold => FontWeight::BOLD,
        wire::Weight::ExtraBold => FontWeight::EXTRA_BOLD,
        wire::Weight::Black => FontWeight::BLACK,
    }
}

pub(super) fn text_options(
    mut element: Div,
    font: wire::Font,
    align: Option<wire::AlignX>,
    options: &wire::TextOptions,
) -> Div {
    element = element.font_weight(font_weight(font.weight));
    if font.monospace {
        element = crate::shell::mono_family(element);
    }
    if let Some(font) = &options.font {
        // The generic families name no face this app registered, and a family
        // that does not resolve takes the weight and the slant down with it:
        // a run asking for bold sans-serif came back as plain body text.
        // Every generic but the monospace one is the app's own text face,
        // which is the one with the weights.
        let family = match &font.family {
            wire::FontFamily::Named(name) => name.clone(),
            wire::FontFamily::Monospace => design::fonts::FAMILY_MONO.into(),
            wire::FontFamily::Serif
            | wire::FontFamily::SansSerif
            | wire::FontFamily::Cursive
            | wire::FontFamily::Fantasy => design::fonts::FAMILY_UI.into(),
        };
        element = crate::shell::with_family(element, family).font_weight(font_weight(font.weight));
        if font.style != wire::FontStyle::Normal {
            element = element.italic();
        }
    }
    element = match align {
        Some(wire::AlignX::Center) => element.text_center(),
        Some(wire::AlignX::Right) => element.text_right(),
        _ => element,
    };
    if let Some(height) = options.line_height {
        element = match height {
            wire::LineHeight::Relative(value) => element.line_height(relative(value)),
            wire::LineHeight::Absolute(value) => element.line_height(px(value)),
        };
    }
    if options.wrapping == Some(wire::Wrapping::None) {
        // A non-wrapping label still owns only its allocated box. In a row,
        // painting the full intrinsic line would cover the following fields.
        element = element.truncate();
    } else {
        // Native buttons set inherited nowrap. A wire label's wrapping is
        // independent of its interactive ancestor, including the default.
        element = element.whitespace_normal();
    }
    element
}

pub(super) fn horizontal_align(element: Div, alignment: Option<wire::AlignX>) -> Div {
    match alignment {
        Some(wire::AlignX::Left) => element.justify_start(),
        Some(wire::AlignX::Center) => element.justify_center(),
        Some(wire::AlignX::Right) => element.justify_end(),
        None => element,
    }
}

pub(super) fn vertical_align(element: Div, alignment: Option<wire::AlignY>) -> Div {
    match alignment {
        Some(wire::AlignY::Top) => element.items_start(),
        Some(wire::AlignY::Center) => element.items_center(),
        Some(wire::AlignY::Bottom) => element.items_end(),
        None => element,
    }
}

pub(super) fn cross_align(element: Div, alignment: Option<wire::AlignX>) -> Div {
    match alignment {
        Some(wire::AlignX::Left) => element.items_start(),
        Some(wire::AlignX::Center) => element.items_center(),
        Some(wire::AlignX::Right) => element.items_end(),
        None => element,
    }
}

pub(super) fn justify(element: Div, alignment: wire::FlexContentAlignment) -> Div {
    match alignment {
        wire::FlexContentAlignment::Start | wire::FlexContentAlignment::FlexStart => {
            element.justify_start()
        }
        wire::FlexContentAlignment::End | wire::FlexContentAlignment::FlexEnd => {
            element.justify_end()
        }
        wire::FlexContentAlignment::Center => element.justify_center(),
        wire::FlexContentAlignment::SpaceBetween => element.justify_between(),
        wire::FlexContentAlignment::SpaceAround => element.justify_around(),
        wire::FlexContentAlignment::SpaceEvenly => element.justify_evenly(),
        wire::FlexContentAlignment::Stretch => element,
    }
}

pub(super) fn align_items(element: Div, alignment: wire::FlexItemAlignment) -> Div {
    match alignment {
        wire::FlexItemAlignment::Start | wire::FlexItemAlignment::FlexStart => {
            element.items_start()
        }
        wire::FlexItemAlignment::End | wire::FlexItemAlignment::FlexEnd => element.items_end(),
        wire::FlexItemAlignment::Center => element.items_center(),
        wire::FlexItemAlignment::Baseline => element.items_baseline(),
        wire::FlexItemAlignment::Stretch => element.items_stretch(),
    }
}

pub(super) fn shadows(element: Div, shadow: wire::Shadow) -> Div {
    let Some(color) = shadow.color else {
        return element;
    };
    element.shadow(vec![BoxShadow {
        color: rgba(color),
        offset: point(
            px(shadow.x.unwrap_or_default()),
            px(shadow.y.unwrap_or_default()),
        ),
        blur_radius: px(shadow.blur.unwrap_or_default()),
        spread_radius: px(0.0),
        inset: false,
    }])
}

pub(super) fn object_fit(fit: Option<wire::ContentFit>) -> ObjectFit {
    match fit {
        Some(wire::ContentFit::Cover) => ObjectFit::Cover,
        Some(wire::ContentFit::Fill) => ObjectFit::Fill,
        Some(wire::ContentFit::None) => ObjectFit::None,
        Some(wire::ContentFit::ScaleDown) => ObjectFit::ScaleDown,
        Some(wire::ContentFit::Contain) | None => ObjectFit::Contain,
    }
}
