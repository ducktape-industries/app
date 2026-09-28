use super::*;

pub(super) fn transaction_id(
    instance: u64,
    state: &EditorDocumentRef,
    sequence: u64,
) -> view_wire::EditorTransactionId {
    view_wire::EditorTransactionId {
        instance,
        document: state.document.clone(),
        reset: state.reset,
        sequence,
        attempt: 0,
        text_revision: state.text_revision,
        revision: state.revision,
    }
}

pub(super) fn offset(text: &str, position: view_wire::EditorPosition) -> usize {
    let Some(line) = view_wire::editor_lines(text).nth(position.line as usize) else {
        return text.len();
    };
    let start = line.as_ptr() as usize - text.as_ptr() as usize;
    start + (position.column as usize).min(line.len())
}

pub(super) fn position(text: &str, mut at: usize) -> view_wire::EditorPosition {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let (index, source) = view_wire::editor_lines(text)
        .enumerate()
        .take_while(|(_, line)| line.as_ptr() as usize - text.as_ptr() as usize <= at)
        .last()
        .expect("editor has at least one logical line");
    let start = source.as_ptr() as usize - text.as_ptr() as usize;
    view_wire::EditorPosition {
        line: index as u32,
        column: (at - start).min(source.len()) as u32,
    }
}

pub(super) fn native_edit(
    before: &str,
    previous: view_wire::EditorCursor,
    after: &str,
    next: view_wire::EditorCursor,
    kind: view_wire::EditorEditKind,
) -> Result<NativeEdit, String> {
    let patches = editor_changed_span(before, after)
        .map_err(|error| format!("native editor edit refused: {error:?}"))?;
    let caret = offset(before, previous.position) as isize;
    let patch = patches
        .into_iter()
        .next()
        .unwrap_or(view_wire::EditorPatch {
            start_byte: caret as u32,
            end_byte: caret as u32,
            replacement: String::new(),
        });
    Ok(NativeEdit {
        start: patch.start_byte as isize - caret,
        end: patch.end_byte as isize - caret,
        caret: offset(after, next.position) as isize - patch.start_byte as isize,
        anchor: next
            .selection
            .map(|p| offset(after, p) as isize - patch.start_byte as isize),
        replacement: patch.replacement,
        kind,
    })
}

pub(super) fn apply_native(
    text: &str,
    cursor: view_wire::EditorCursor,
    edit: &NativeEdit,
) -> Result<(Vec<view_wire::EditorPatch>, view_wire::EditorCursor), String> {
    let caret = offset(text, cursor.position) as isize;
    let start = caret
        .checked_add(edit.start)
        .filter(|n| *n >= 0)
        .ok_or("queued editor range before document")? as usize;
    let end = caret
        .checked_add(edit.end)
        .filter(|n| *n >= 0)
        .ok_or("queued editor range before document")? as usize;
    let valid = start <= end
        && end <= text.len()
        && text.is_char_boundary(start)
        && text.is_char_boundary(end);
    if !valid {
        return Err("queued editor range no longer valid; input retained".into());
    }
    let patch = view_wire::EditorPatch {
        start_byte: start as u32,
        end_byte: end as u32,
        replacement: edit.replacement.clone(),
    };
    let mut next = String::with_capacity(text.len() - (end - start) + edit.replacement.len());
    next.push_str(&text[..start]);
    next.push_str(&edit.replacement);
    next.push_str(&text[end..]);
    let mut cursor = view_wire::EditorCursor {
        position: position(&next, (start as isize + edit.caret).max(0) as usize),
        selection: edit
            .anchor
            .map(|anchor| position(&next, (start as isize + anchor).max(0) as usize)),
    };
    cursor.clamp(&next);
    let patches = if start == end && edit.replacement.is_empty() {
        vec![]
    } else {
        vec![patch]
    };
    Ok((patches, cursor))
}

pub(super) fn native_key(
    text: &str,
    cursor: view_wire::EditorCursor,
    key: &view_wire::keyboard::KeyState,
) -> (Vec<view_wire::EditorPatch>, view_wire::EditorCursor) {
    use view_wire::keyboard::{Key, Named};
    let caret = offset(text, cursor.position);
    let anchor = cursor.selection.map_or(caret, |p| offset(text, p));
    let mut start = caret.min(anchor);
    let mut end = caret.max(anchor);
    // a span one patch may remove: never half a character, a grapheme or a
    // two-byte line terminator
    let removable = |start: usize, end: usize| {
        let patch = view_wire::EditorPatch {
            start_byte: start as u32,
            end_byte: end as u32,
            replacement: String::new(),
        };
        text.is_char_boundary(start)
            && text.is_char_boundary(end)
            && view_wire::patched_editor_text(text, &[patch], view_wire::EditorCursor::default())
                .is_ok()
    };
    let replacement = match &key.key {
        Key::Named(Named::Enter) => "\n",
        Key::Named(Named::Tab) => "\t",
        Key::Named(Named::Backspace) => {
            if start == end && start > 0 {
                start -= 1;
                while start > 0 && !removable(start, end) {
                    start -= 1;
                }
            }
            ""
        }
        // forward: what follows the caret, and nothing at the end
        Key::Named(Named::Delete) => {
            if start == end && end < text.len() {
                end += 1;
                while end < text.len() && !removable(start, end) {
                    end += 1;
                }
            }
            ""
        }
        // the caret to the same column a line up or down, or to the start or
        // end of the text past the first or last line — no patch, and the
        // selection goes
        Key::Named(named @ (Named::ArrowUp | Named::ArrowDown)) => {
            let here = position(text, caret);
            let last = view_wire::editor_lines(text).count().saturating_sub(1) as u32;
            let at = match named {
                Named::ArrowUp if here.line == 0 => 0,
                Named::ArrowUp => offset(
                    text,
                    view_wire::EditorPosition {
                        line: here.line - 1,
                        ..here
                    },
                ),
                _ if here.line >= last => text.len(),
                _ => offset(
                    text,
                    view_wire::EditorPosition {
                        line: here.line + 1,
                        ..here
                    },
                ),
            };
            let caret_at = |at: usize| view_wire::EditorCursor {
                position: position(text, at),
                selection: None,
            };
            // the same column a line away can fall inside a grapheme: back to
            // its start, which the wire takes as a caret
            let mut at = at;
            while at > 0 && view_wire::patched_editor_text(text, &[], caret_at(at)).is_err() {
                at -= 1;
            }
            return (Vec::new(), caret_at(at));
        }
        // Escape, a cut with nothing selected, and any key the host has no
        // native default for do nothing — committed with no patch, so the
        // guest's editor is not left waiting. Stopping the view over a key it
        // handed the host stopped a view's composer on an ordinary keystroke.
        _ => return (Vec::new(), cursor),
    };
    let mut next = text[..start].to_owned();
    next.push_str(replacement);
    next.push_str(&text[end..]);
    let cursor = view_wire::EditorCursor {
        position: position(&next, start + replacement.len()),
        selection: None,
    };
    (
        vec![view_wire::EditorPatch {
            start_byte: start as u32,
            end_byte: end as u32,
            replacement: replacement.into(),
        }],
        cursor,
    )
}
