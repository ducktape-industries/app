//! What a seat draws where its view is not: the load's stage over a
//! skeleton, or why there is none, with Retry where a retry helps.
use super::*;

/// What a pane draws where its view is not: the load's stage over a skeleton
/// of a view, or why there is none — named, with Retry where a retry helps.
/// Compared: the seat notifies only when the words move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Standin {
    pub(crate) title: Option<&'static str>,
    pub(crate) words: String,
    pub(crate) loading: bool,
    pub(crate) retry: bool,
}

impl From<String> for Standin {
    fn from(words: String) -> Self {
        Standin {
            title: None,
            words,
            loading: false,
            retry: false,
        }
    }
}

impl From<&Failure> for Standin {
    fn from(failure: &Failure) -> Self {
        Standin {
            title: Some(failure.title()),
            words: failure.to_string(),
            loading: false,
            retry: true,
        }
    }
}

/// The words a loading pane says for its stage.
pub(super) fn stage_words(slot: &Slot) -> String {
    let size = |bytes: u64| match bytes < 1_000_000 {
        true => format!("{} KB", bytes.div_ceil(1000)),
        false => format!("{:.1} MB", bytes as f64 / 1e6),
    };
    match slot {
        Slot::Fetching {
            received,
            total: Some(total),
        } => format!(
            "Fetching the view — {} of {}",
            size(*received),
            size(*total)
        ),
        Slot::Fetching { received, .. } => format!("Fetching the view — {}", size(*received)),
        Slot::Compiling => "Compiling the view…".into(),
        _ => "Loading the view…".into(),
    }
}

impl Standin {
    /// THE ONE STAND-IN every pane draws where its view is not, native, so
    /// it draws before any wasm exists: a loading view says its stage over
    /// a skeleton laid out as a view lays itself out — a heading, then
    /// rows, from the top of the full pane the view will take, so nothing
    /// moves when it seats — and a failed one says what failed, why, and
    /// offers Retry, which asks the seat's view for again (`retry`) and so
    /// turns the seat.
    pub(crate) fn element(
        &self,
        module: &'static str,
        instance: u64,
        mark: gpui_kit::ElementId,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::Button;
        use gpui_kit::{
            FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, Role,
            StatefulInteractiveElement as _, Styled as _, div, px, relative,
        };
        let Standin {
            title,
            words,
            loading,
            retry: offers_retry,
        } = self.clone();
        // a stable id per kind, so the tree tells a load on its way from a
        // view that is not there
        let (id, words_id) = match loading {
            true => ("view-loading", "view-loading-stage"),
            false => ("view-unavailable", "view-unavailable-reason"),
        };
        // a failure is announced like any other alert (the connect
        // screen's connect-error, the key screen's unlock-error): a name,
        // not just a role, or the AX door's compact filter drops it
        let alert_label = match &title {
            Some(title) => format!("{title}: {words}"),
            None => words.clone(),
        };
        // the whole sentence, wrapped inside the pane: one line wider than
        // the pane is centred off both edges, and a reader loses its start
        // and its end — what failed, and what to do about it
        let reason = div().id(id).max_w_full().flex().flex_col().gap_2();
        let reason = match loading {
            true => reason,
            false => reason.role(Role::Alert).aria_label(alert_label),
        };
        let reason = reason
            .children(title.map(|title| {
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(gpui_kit::Text::new(
                        "view-unavailable-title".into(),
                        title.into(),
                    ))
            }))
            .child(gpui_kit::Text::new(words_id.into(), words.into()))
            .children(offers_retry.then(|| {
                Button::new("view-retry")
                    .label("Retry")
                    .outline()
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        drop(retry(module, instance));
                    })
            }));
        #[cfg(test)]
        let reason = {
            use gpui_kit::test::TestSupportExt as _;
            reason.test_support()
        };
        let pane = div().id(mark).size_full().flex().flex_col().p_4();
        if !loading {
            return pane
                .items_center()
                .justify_center()
                .child(reason)
                .into_any_element();
        }
        let faint = gpui_kit::hsla(0., 0., 0.5, 0.14);
        let bar = |width: f32, height: f32| {
            div()
                .w(relative(width))
                .h(px(height))
                .rounded(px(4.))
                .bg(faint)
        };
        pane.gap_3()
            .child(bar(0.3, 22.))
            .child(div().text_size(px(12.)).opacity(0.7).child(reason))
            .children([0.9, 0.7, 0.8, 0.55, 0.85, 0.65].map(|width| bar(width, 14.)))
            .into_any_element()
    }
}
