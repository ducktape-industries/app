use gpui_kit::{AnyElement, AnyView, IntoElement as _, StyleRefinement, Window};

/// `view` as a cached element laid out at `size`, unless assistive
/// technology is listening. While one listens the view renders in full on
/// every frame, as before the pinned fork carried a cached view's
/// accessibility nodes through reuse: caching under a listener waits until
/// the a11y census proves those nodes in this app.
pub(crate) fn cached_unless_a11y(
    view: AnyView,
    size: StyleRefinement,
    window: &Window,
) -> AnyElement {
    match window.is_a11y_active() {
        true => view.into_any_element(),
        false => view.cached(size).into_any_element(),
    }
}
