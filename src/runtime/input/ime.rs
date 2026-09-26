use super::wire;
use std::ops::Range;

/// Native IME ranges are UTF-16. The wire only carries UTF-8 preedit offsets.
#[derive(Clone)]
pub(crate) struct ImeState {
    start: usize,
    content: String,
    selection: (u32, u32),
}
pub(crate) fn ime_events(
    previous: &mut Option<ImeState>,
    text: &str,
    marked: Option<Range<usize>>,
    cursor: usize,
    selected: Range<usize>,
) -> Vec<wire::Event> {
    use wire::events::InputMethod as I;
    let byte = |wanted: usize| {
        let mut units = 0;
        for (offset, ch) in text.char_indices() {
            if units >= wanted {
                return offset;
            }
            units += ch.len_utf16();
        }
        text.len()
    };
    let mut events = Vec::new();
    match marked {
        Some(range) => {
            let start = byte(range.start);
            let end = byte(range.end);
            let content = text[start..end].to_owned();
            let selection = (
                selected.start.saturating_sub(start).min(content.len()) as u32,
                selected.end.saturating_sub(start).min(content.len()) as u32,
            );
            if content.len() > wire::MAX_STRING_BYTES {
                return Vec::new();
            }
            if previous.is_none() {
                events.push(I::Opened);
            }
            let changed = previous.as_ref().is_none_or(|old| {
                old.start != start || old.content != content || old.selection != selection
            });
            if changed {
                events.push(I::Preedit {
                    selection: Some(selection),
                    content: content.clone(),
                });
            }
            *previous = Some(ImeState {
                start,
                content,
                selection,
            });
        }
        None => {
            let Some(old) = previous.take() else {
                return Vec::new();
            };
            if let Some(content) = text.get(old.start..cursor)
                && !content.is_empty()
                && content.len() <= wire::MAX_STRING_BYTES
            {
                events.push(I::Commit(content.to_owned()));
            }
            events.push(I::Closed);
        }
    }
    events
        .into_iter()
        .map(|event| wire::Event::Observation {
            event: wire::events::Event::InputMethod(event),
            captured: true,
        })
        .collect()
}
