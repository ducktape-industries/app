//! The style table a test draws with, and frames as a view sends them: what
//! the renderer's own tests and the shells' tests build trees from
//! (feature `test-support`, on for every test build).

use super::*;
use std::cell::RefCell;

thread_local! {
    /// The style table of the test on this thread: every style one of its
    /// nodes names, as the view numbers them and the host holds them.
    static STYLES: RefCell<(wire::Interner, wire::Styles)> = Default::default();
}

/// The id `style` has in this test's table, which holds it from here on.
pub fn test_style(style: impl std::borrow::Borrow<gpui_kit::StyleRefinement>) -> wire::StyleId {
    STYLES.with_borrow_mut(|(view, host)| {
        let id = view.intern(style.borrow());
        host.extend(view.unsent()).expect("a style the host takes");
        id
    })
}

/// The id of the style that sets nothing.
pub fn plain_style() -> wire::StyleId {
    test_style(gpui_kit::StyleRefinement::default())
}

/// This test's table, as far as it has named styles.
pub fn test_styles() -> wire::Styles {
    STYLES.with_borrow(|(_, host)| host.clone())
}

/// `frame` as a view sends it: a whole one carries this test's table.
pub fn sent(mut frame: wire::Frame) -> wire::Frame {
    if frame.root.is_some() {
        let held = test_styles();
        frame.styles = (0..held.len() as u32)
            .map(|id| wire::Style::new(&held[wire::StyleId(id)]))
            .collect();
    }
    frame
}

/// A whole `frame` through the sanitizer, as the host takes one a view
/// sent.
pub fn sanitize_whole(frame: &mut wire::Frame) -> Result<wire::SanitizeReport, wire::Refused> {
    *frame = sent(std::mem::take(frame));
    wire::sanitize(frame, &mut wire::Styles::default())
}

/// Tooltip answers as a seat hands them to its tree: each with the table
/// of the frame it came in, here this test's.
pub fn answered(
    responses: impl IntoIterator<Item = wire::TooltipResponse>,
) -> Vec<(wire::TooltipResponse, wire::Styles)> {
    // the answers first: building one may name a style
    let responses: Vec<_> = responses.into_iter().collect();
    let styles = test_styles();
    responses
        .into_iter()
        .map(|response| (response, styles.clone()))
        .collect()
}

/// A test's tree is drawn with the test's table.
impl From<wire::Node> for Tree {
    fn from(root: wire::Node) -> Self {
        Self {
            root,
            styles: test_styles(),
        }
    }
}

pub fn draw_frame(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    window.draw(cx).clear(cx);
}

pub fn refresh_and_draw(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    window.refresh();
    window.draw(cx).clear(cx);
}
