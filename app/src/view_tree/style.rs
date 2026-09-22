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
