//! The pointer's shape over a page: CEF names the cursor the page wants
//! (`DisplayHandler::on_cursor_change`), the page's box shows gpui's.

use cef::sys::cef_cursor_type_t::{self as Cef, *};
use gpui_kit::CursorStyle;

pub(crate) fn style(cursor: cef::CursorType) -> CursorStyle {
    match Cef::from(cursor) {
        CT_CROSS | CT_CELL => CursorStyle::Crosshair,
        CT_HAND => CursorStyle::PointingHand,
        CT_IBEAM => CursorStyle::IBeam,
        CT_VERTICALTEXT => CursorStyle::IBeamCursorForVerticalLayout,
        CT_EASTRESIZE => CursorStyle::ResizeRight,
        CT_WESTRESIZE => CursorStyle::ResizeLeft,
        CT_NORTHRESIZE => CursorStyle::ResizeUp,
        CT_SOUTHRESIZE => CursorStyle::ResizeDown,
        CT_EASTWESTRESIZE => CursorStyle::ResizeLeftRight,
        CT_NORTHSOUTHRESIZE => CursorStyle::ResizeUpDown,
        CT_NORTHWESTRESIZE | CT_SOUTHEASTRESIZE | CT_NORTHWESTSOUTHEASTRESIZE => {
            CursorStyle::ResizeUpLeftDownRight
        }
        CT_NORTHEASTRESIZE | CT_SOUTHWESTRESIZE | CT_NORTHEASTSOUTHWESTRESIZE => {
            CursorStyle::ResizeUpRightDownLeft
        }
        CT_COLUMNRESIZE => CursorStyle::ResizeColumn,
        CT_ROWRESIZE => CursorStyle::ResizeRow,
        CT_GRAB | CT_MOVE => CursorStyle::OpenHand,
        CT_GRABBING | CT_DND_MOVE => CursorStyle::ClosedHand,
        CT_NOTALLOWED | CT_NODROP | CT_DND_NONE => CursorStyle::OperationNotAllowed,
        CT_ALIAS | CT_DND_LINK => CursorStyle::DragLink,
        CT_COPY | CT_DND_COPY => CursorStyle::DragCopy,
        CT_CONTEXTMENU => CursorStyle::ContextualMenu,
        _ => CursorStyle::Arrow,
    }
}

#[cfg(test)]
mod tests {
    use super::style;
    use cef::CursorType;
    use gpui_kit::CursorStyle;

    #[test]
    fn a_page_cursor_is_the_nearest_gpui_has() {
        for (cef, gpui) in [
            (CursorType::POINTER, CursorStyle::Arrow),
            (CursorType::HAND, CursorStyle::PointingHand),
            (CursorType::IBEAM, CursorStyle::IBeam),
            (CursorType::CROSS, CursorStyle::Crosshair),
            (CursorType::NOTALLOWED, CursorStyle::OperationNotAllowed),
            (CursorType::EASTWESTRESIZE, CursorStyle::ResizeLeftRight),
            (CursorType::NORTHSOUTHRESIZE, CursorStyle::ResizeUpDown),
            (
                CursorType::NORTHWESTSOUTHEASTRESIZE,
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                CursorType::NORTHEASTSOUTHWESTRESIZE,
                CursorStyle::ResizeUpRightDownLeft,
            ),
            (CursorType::COLUMNRESIZE, CursorStyle::ResizeColumn),
            (CursorType::MOVE, CursorStyle::OpenHand),
            (CursorType::GRABBING, CursorStyle::ClosedHand),
            (CursorType::WAIT, CursorStyle::Arrow),
            (CursorType::PROGRESS, CursorStyle::Arrow),
            (CursorType::NONE, CursorStyle::Arrow),
        ] {
            assert_eq!(style(cef), gpui, "{cef:?}");
        }
    }
}
