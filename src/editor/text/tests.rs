use super::*;

#[cfg(test)]
fn editor_path() -> crate::render::AuthoredPath {
    vec![wire::ElementIdWire::Name("document".into())]
}

#[cfg(test)]
fn store_with(
    name: &str,
    text: &str,
    claims: Vec<wire::EditorKeyClaim>,
    placeholder: &str,
) -> EditorStore {
    let store = EditorStore::new(91);
    let reference = wire::editor_document::EditorDocumentRef {
        document: name.into(),
        reset: 1,
        revision: 0,
        text_revision: 0,
        byte_len: text.len() as u32,
        cursor: wire::EditorCursor {
            position: position(text, text.len()),
            selection: None,
        },
    };
    let mut locked = store.lock();
    locked.fields.insert(
        editor_path(),
        super::super::Field {
            reference: reference.clone(),
            handler: 1,
            editable: true,
            placeholder: placeholder.to_owned(),
            binding: Some(Box::new(wire::EditorBinding {
                on_request: 2,
                on_event: 3,
                claims,
            })),
        },
    );
    locked.documents.insert(
        reference.document.clone(),
        super::super::Document {
            reference,
            text: Some(Arc::from(text)),
            queue: Default::default(),
            queued_bytes: 0,
            phase: super::super::Phase::Ready,
        },
    );
    drop(locked);
    store
}

/// Settle every queued edit against the document, the way a guest that accepts
/// what the field did would.
#[cfg(test)]
fn settle(store: &EditorStore, name: &str) {
    let mut locked = store.lock();
    while !locked.documents[name].queue.is_empty() {
        let accepted = locked.documents[name].reference.clone();
        locked.fields.get_mut(&editor_path()).unwrap().reference = accepted;
        locked.acknowledge();
        locked.pump();
        assert!(locked.fault.is_none(), "{:?}", locked.fault);
    }
}

/// The field carries the guest's placeholder, follows it when the guest
/// changes it, and takes what is typed into it.
#[cfg(test)]
#[gpui_kit::test]
fn an_empty_field_wears_the_guests_placeholder_and_takes_what_is_typed(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    cx.update(gpui_kit::init);
    let store = store_with("empty", "", Vec::new(), "Start writing");
    let window = cx.open_window(gpui_kit::size(px(400.), px(200.)), |window, cx| {
        TextEditor::new(editor_path(), store.clone(), window, cx)
    });
    let editor = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        let input = editor.read(cx).input.clone();
        assert!(input.read(cx).value().is_empty());
        assert_eq!(
            input.read(cx).presentation().placeholder().as_ref(),
            "Start writing"
        );
        input.read(cx).focus_handle(cx).focus(window, cx);
        window.render_frame(cx);
    });
    store
        .lock()
        .fields
        .get_mut(&editor_path())
        .unwrap()
        .placeholder = "새 문서".into();
    native.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.sync(window, cx));
        window.render_frame(cx);
        assert_eq!(
            editor
                .read(cx)
                .input
                .read(cx)
                .presentation()
                .placeholder()
                .as_ref(),
            "새 문서"
        );
        window.input("Written text", cx);
    });
    native.run_until_parked();
    native.update(|window, cx| {
        window.render_frame(cx);
        let editor = editor.read(cx);
        assert_eq!(editor.input.read(cx).value().as_ref(), "Written text");
        assert_eq!(editor.preview.as_ref(), "Written text");
        assert!(store.lock().fault.is_none());
        window.blur(cx);
    });
}

/// Shift and an arrow reach past the line they started on. This is the whole
/// reason the document is one field: a selection that stops at the newline is
/// a selection that cannot take a paragraph.
#[cfg(test)]
#[gpui_kit::test]
fn shift_and_an_arrow_select_across_the_lines_of_one_document(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    cx.update(gpui_kit::init);
    let store = store_with("lines", "one\ntwo\nthree", Vec::new(), "");
    let window = cx.open_window(gpui_kit::size(px(400.), px(200.)), |window, cx| {
        TextEditor::new(editor_path(), store.clone(), window, cx)
    });
    let editor = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        editor.update(cx, |editor, cx| {
            editor.input.read(cx).focus_handle(cx).focus(window, cx)
        });
        window.render_frame(cx);
        window.dispatch_keystroke(Keystroke::parse("shift-up").unwrap(), cx);
        window.dispatch_keystroke(Keystroke::parse("shift-up").unwrap(), cx);
    });
    native.run_until_parked();
    native.update(|window, cx| window.render_frame(cx));
    editor.read_with(&native, |editor, cx| {
        let selected = editor.input.read(cx).selected_range();
        assert!(
            editor.preview[selected.start..selected.end].contains('\n'),
            "a selection that took two shift-ups spans the newlines it crossed"
        );
        let anchor = editor
            .cursor
            .selection
            .expect("the selection reaches the guest");
        assert_ne!(
            anchor.line, editor.cursor.position.line,
            "the guest is told the selection crosses lines"
        );
    });
}

/// Backspace at the head of a line takes the newline before it and joins the
/// two lines — the ordinary way any text box works.
#[cfg(test)]
#[gpui_kit::test]
fn backspace_at_the_head_of_a_line_joins_it_to_the_one_above(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    cx.update(gpui_kit::init);
    let store = store_with("join", "one\ntwo", Vec::new(), "");
    let window = cx.open_window(gpui_kit::size(px(400.), px(200.)), |window, cx| {
        TextEditor::new(editor_path(), store.clone(), window, cx)
    });
    let editor = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        editor.update(cx, |editor, cx| {
            editor.input.read(cx).focus_handle(cx).focus(window, cx)
        });
        window.render_frame(cx);
        // The caret starts at the end of "two"; Home puts it at the head.
        window.dispatch_keystroke(Keystroke::parse("home").unwrap(), cx);
        window.dispatch_keystroke(Keystroke::parse("backspace").unwrap(), cx);
    });
    native.run_until_parked();
    settle(&store, "join");
    native.update(|window, cx| window.render_frame(cx));
    editor.read_with(&native, |editor, _| {
        assert_eq!(editor.preview.as_ref(), "onetwo");
    });
    assert_eq!(
        store.lock().documents["join"].text.as_deref(),
        Some("onetwo"),
        "the joined document reaches the guest"
    );
}

/// A claimed chord is the guest's and never the field's; everything else is
/// the field's and never the guest's.
#[cfg(test)]
#[gpui_kit::test]
fn the_guest_hears_the_chords_it_claimed_and_no_others(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    cx.update(gpui_kit::init);
    let store = store_with(
        "claims",
        "text",
        vec![
            wire::EditorKeyClaim {
                key: wire::keyboard::Key::Character("z".into()),
                modifiers: Default::default(),
                command: true,
            },
            wire::EditorKeyClaim {
                key: wire::keyboard::Key::Named(wire::keyboard::Named::Shift),
                modifiers: Default::default(),
                command: false,
            },
        ],
        "",
    );
    let window = cx.open_window(gpui_kit::size(px(400.), px(200.)), |window, cx| {
        TextEditor::new(editor_path(), store.clone(), window, cx)
    });
    let editor = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        editor.update(cx, |editor, cx| {
            editor.input.read(cx).focus_handle(cx).focus(window, cx)
        });
        window.render_frame(cx);
    });
    store.drain();
    // A lone Shift tap reaches keystroke interceptors since gpui-pre 0.3.7
    // (zed ba42ab9d9). It is no key to claim: taking it would also keep its
    // release from the window's modifier listeners.
    native.simulate_modifiers_change(gpui_kit::Modifiers::shift());
    native.simulate_modifiers_change(gpui_kit::Modifiers::none());
    let tapped = store.drain();
    assert!(
        !tapped
            .iter()
            .any(|event| matches!(event, wire::Event::EditorRequest { .. })),
        "a bare modifier is never the guest's: {tapped:?}"
    );
    let undo = if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    };
    native.update(|window, cx| window.dispatch_keystroke(Keystroke::parse(undo).unwrap(), cx));
    let claimed = store.drain();
    assert!(
        claimed.iter().any(
            |event| matches!(event, wire::Event::EditorRequest { request, .. }
            if matches!(&request.input, wire::EditorRequestInput::Key { key, .. }
                if key.key == wire::keyboard::Key::Character("z".into())))
        ),
        "undo must reach guest history, not the field's own undo stack: {claimed:?}"
    );
    native.update(|window, cx| window.dispatch_keystroke(Keystroke::parse("left").unwrap(), cx));
    let unclaimed = store.drain();
    assert!(
        !unclaimed
            .iter()
            .any(|event| matches!(event, wire::Event::EditorRequest { .. })),
        "an arrow is the field's to answer: {unclaimed:?}"
    );
}

/// A guest cursor whose caret is the earlier end still selects its words;
/// gpui-base 0.7.0 read that backward range as empty and selected nothing.
#[cfg(test)]
#[gpui_kit::test]
fn a_backward_guest_selection_stays_selected(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let text = "hello world";
    let store = store_with("backward", text, Vec::new(), "");
    let backward = wire::EditorCursor {
        position: position(text, 0),
        selection: Some(position(text, 5)),
    };
    {
        let mut locked = store.lock();
        locked
            .fields
            .get_mut(&editor_path())
            .unwrap()
            .reference
            .cursor = backward;
        locked
            .documents
            .get_mut("backward")
            .unwrap()
            .reference
            .cursor = backward;
    }
    let window = cx.open_window(gpui_kit::size(px(400.), px(200.)), |window, cx| {
        TextEditor::new(editor_path(), store.clone(), window, cx)
    });
    let editor = window.root(cx).unwrap();
    let selected = editor.read_with(cx, |editor, cx| editor.input.read(cx).selected_range());
    assert_eq!(selected, 0..5);
}

/// A field the guest will not let anyone write in reports nothing, and keeps
/// the text it was given.
#[cfg(test)]
#[gpui_kit::test]
fn a_readonly_field_reports_no_edit(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    cx.update(gpui_kit::init);
    let store = store_with("readonly", "Read only 한글", Vec::new(), "");
    store
        .lock()
        .fields
        .get_mut(&editor_path())
        .unwrap()
        .editable = false;
    let window = cx.open_window(gpui_kit::size(px(400.), px(200.)), |window, cx| {
        TextEditor::new(editor_path(), store.clone(), window, cx)
    });
    let editor = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        editor.update(cx, |editor, cx| {
            editor.input.read(cx).focus_handle(cx).focus(window, cx)
        });
        window.render_frame(cx);
    });
    store.drain();
    native.update(|window, cx| {
        window.input("no", cx);
        window.dispatch_keystroke(Keystroke::parse("backspace").unwrap(), cx);
    });
    native.run_until_parked();
    editor.read_with(&native, |editor, cx| {
        assert_eq!(editor.preview.as_ref(), "Read only 한글");
        assert!(!editor.input.read(cx).is_editable());
    });
    assert_eq!(
        store.lock().documents["readonly"].text.as_deref(),
        Some("Read only 한글")
    );
}

/// A drag-selection is one caret move per pointer sample, against a queue that
/// drains one item per guest frame and faults the whole view when it fills.
/// Two caret moves in a row compose, so the queue keeps the one in flight and
/// one destination however far the pointer travels.
#[cfg(test)]
#[test]
fn a_drag_through_a_paragraph_does_not_fill_the_queue() {
    let text = "one two three four five six seven eight nine ten";
    let store = store_with("drag", text, Vec::new(), "");
    let reaching = |byte: usize| wire::EditorCursor {
        position: position(text, byte),
        selection: Some(position(text, 0)),
    };
    let mut held = wire::EditorCursor {
        position: position(text, 0),
        selection: None,
    };
    for byte in 1..text.len() {
        let next = reaching(byte);
        store.native(
            &editor_path(),
            text,
            held,
            text,
            next,
            wire::EditorEditKind::Cursor,
        );
        held = next;
    }
    let locked = store.lock();
    assert!(locked.fault.is_none(), "{:?}", locked.fault);
    let queue = &locked.documents["drag"].queue;
    assert!(
        queue.len() <= 2,
        "a drag of {} samples left {} in the queue",
        text.len() - 1,
        queue.len()
    );
}

/// Tab is an indent: typed where the caret stands, and carried across whole
/// lines when a selection covers them. Shift+Tab takes one back, and takes
/// nothing when there is nothing left to take — which is what leaves the key
/// to the focus ring.
#[cfg(test)]
#[test]
fn tab_indents_a_caret_a_block_and_gives_it_back() {
    let typed = indent("ab", 1..1, false).expect("an indent at the caret");
    assert_eq!(typed, ("a  b".to_owned(), 3..3));

    // Two lines selected from the middle of the first to the middle of the
    // second: both move, and both ends of the selection move with them.
    let block = indent("one\ntwo\nthree", 1..5, false).expect("a block indent");
    assert_eq!(block, ("  one\n  two\nthree".to_owned(), 3..9));

    // A selection carried to the head of the next line leaves that line where
    // it is — the writer stopped before it.
    let up_to = indent("one\ntwo", 0..4, false).expect("a block indent");
    assert_eq!(up_to.0, "  one\ntwo");

    let back = indent("  one\n  two", 3..9, true).expect("an outdent");
    assert_eq!(back, ("one\ntwo".to_owned(), 1..5));

    // A caret inside the indentation being taken back lands at the line's
    // head rather than running off it.
    let inside = indent("  one", 1..1, true).expect("an outdent");
    assert_eq!(inside, ("one".to_owned(), 0..0));

    assert_eq!(indent("one\ntwo", 0..7, true), None);
    assert_eq!(indent("\tone", 0..0, true), Some(("one".to_owned(), 0..0)));
}

/// Tab indents; Esc lets go of it, so the next Tab moves the focus on out
/// of the editor; any other key after Esc takes it back (owner, 2026-09-28;
/// AX-022, Help's keys). A guest that claimed Esc (to cancel an edit, say)
/// still hears it.
#[cfg(test)]
#[gpui_kit::test]
fn esc_then_tab_leaves_the_editor_and_any_other_key_takes_tab_back(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;

    /// The editor with one Tab stop after it, under the kit's Root, whose
    /// Tab moves the focus on.
    struct Host {
        editor: Entity<TextEditor>,
        after: gpui_kit::FocusHandle,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(self.editor.clone())
                .child(div().id("after").track_focus(&self.after).size(px(20.)))
        }
    }

    cx.update(gpui_kit::init);
    // Esc the field's (and the view's) own, and Esc a guest claimed
    for claimed in [false, true] {
        let claims = match claimed {
            false => Vec::new(),
            true => vec![wire::EditorKeyClaim {
                key: wire::keyboard::Key::Named(wire::keyboard::Named::Escape),
                modifiers: Default::default(),
                command: false,
            }],
        };
        let store = store_with("tabs", "one", claims, "");
        let mut host = None;
        let window = cx.open_window(gpui_kit::size(px(400.), px(200.)), |window, cx| {
            let made = cx.new(|cx| Host {
                editor: cx.new(|cx| TextEditor::new(editor_path(), store.clone(), window, cx)),
                after: cx.focus_handle().tab_stop(true),
            });
            host = Some(made.clone());
            gpui_kit::component::Root::new(made, window, cx)
        });
        let host = host.unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let press = |native: &mut gpui_kit::VisualTestContext, keys: &[&str]| {
            native.update(|window, cx| {
                for key in keys {
                    window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
                    window.render_frame(cx);
                }
            });
            native.run_until_parked();
        };
        // the editor's text, and whether it has the keys
        let read = |native: &mut gpui_kit::VisualTestContext| {
            native.update(|window, cx| {
                window.render_frame(cx);
                let editor = host.read(cx).editor.read(cx);
                (
                    editor.input.read(cx).value().to_string(),
                    editor.is_focused(window, cx),
                )
            })
        };
        native.update(|window, cx| {
            window.render_frame(cx);
            let editor = host.read(cx).editor.read(cx);
            editor.input.read(cx).focus_handle(cx).focus(window, cx);
            window.render_frame(cx);
        });

        press(&mut native, &["tab"]);
        assert_eq!(read(&mut native), ("one  ".to_owned(), true), "Tab indents");

        // the guest takes the indent, so the field is free to hear it again
        settle(&store, "tabs");
        store.drain();
        press(&mut native, &["escape", "tab"]);
        let heard = store.drain();
        assert_eq!(
            heard.iter().any(
                |event| matches!(event, wire::Event::EditorRequest { request, .. }
                if matches!(&request.input, wire::EditorRequestInput::Key { key, .. }
                    if key.key == wire::keyboard::Key::Named(wire::keyboard::Named::Escape)))
            ),
            claimed,
            "the guest hears Esc when it claimed it: {heard:?}"
        );
        assert_eq!(
            read(&mut native),
            ("one  ".to_owned(), false),
            "Esc, then Tab moves on"
        );
        assert!(native.update(|window, cx| host.read(cx).after.is_focused(window)));

        native.update(|window, cx| {
            let editor = host.read(cx).editor.read(cx);
            editor.input.read(cx).focus_handle(cx).focus(window, cx);
            window.render_frame(cx);
        });
        press(&mut native, &["escape", "a"]);
        let (typed, _) = read(&mut native);
        assert_eq!(typed, "one  a", "the letter is typed");
        press(&mut native, &["tab"]);
        assert_eq!(
            read(&mut native),
            ("one  a  ".to_owned(), true),
            "a letter after Esc takes Tab back"
        );

        // A lone Shift tap is a keystroke since gpui-pre 0.3.7 (zed
        // ba42ab9d9); it is not a key the writer typed, so Esc's leave holds.
        press(&mut native, &["escape"]);
        native.simulate_modifiers_change(gpui_kit::Modifiers::shift());
        native.simulate_modifiers_change(gpui_kit::Modifiers::none());
        press(&mut native, &["tab"]);
        assert_eq!(
            read(&mut native),
            ("one  a  ".to_owned(), false),
            "Esc, a Shift tap, then Tab still moves on"
        );
        native.update(|window, cx| window.blur(cx));
    }
}

/// The editor, a field as a view mounts it, then one button, under the
/// kit's Root; when `late`, one more stop below the window's edge, which
/// no snapshot shows. The box around them holds the keys.
struct WalkHost {
    held: gpui_kit::FocusHandle,
    editor: Entity<TextEditor>,
    after: gpui_kit::FocusHandle,
    late: Option<gpui_kit::FocusHandle>,
}

impl Render for WalkHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::StatefulInteractiveElement as _;
        let button = |id: &'static str, label: &'static str, handle: &gpui_kit::FocusHandle| {
            div()
                .id(id)
                .role(gpui_kit::Role::Button)
                .aria_label(label)
                .track_focus(handle)
                .on_click(|_, _, _| {})
                .size(px(40.))
        };
        div()
            .id("host")
            .track_focus(&self.held)
            .relative()
            .size_full()
            .child(div().h(px(100.)).child(self.editor.clone()))
            .child(button("after", "After", &self.after))
            .children(
                self.late
                    .as_ref()
                    .map(|late| button("late", "Late", late).absolute().top(px(1000.))),
            )
    }
}

/// The door's Tab walk over a [`WalkHost`] holding "one", from the editor
/// when `in_editor`, else from the box: the report, the focused ids of
/// each snapshot, and the text the walk left.
fn door_walk(
    cx: &mut gpui_kit::TestAppContext,
    in_editor: bool,
    late: bool,
) -> (crate::ax::audit::Report, Vec<Vec<String>>, String) {
    use gpui_kit::test::TestWindowExt as _;
    cx.update(gpui_kit::init);
    let store = store_with("walk", "one", Vec::new(), "");
    let mut host = None;
    let window = cx.open_window(gpui_kit::size(px(400.), px(200.)), |window, cx| {
        let editor = cx.new(|cx| {
            let mut editor = TextEditor::new(editor_path(), store.clone(), window, cx);
            let field = crate::render::Accessible {
                role: Some(gpui_kit::Role::MultilineTextInput),
                name: Some("Message".into()),
                ..Default::default()
            };
            editor.set_accessible(field, cx);
            editor
        });
        let made = cx.new(|cx| WalkHost {
            held: cx.focus_handle(),
            editor,
            after: cx.focus_handle().tab_stop(true),
            late: late.then(|| cx.focus_handle().tab_stop(true)),
        });
        host = Some(made.clone());
        gpui_kit::component::Root::new(made, window, cx)
    });
    let host = host.unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let snap = |window: &mut Window, cx: &mut App| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        crate::ax::snapshot("w", window, false)
    };
    let (report, focus) = native.update(|window, cx| {
        window.render_frame(cx);
        let host = host.read(cx);
        match in_editor {
            true => host.editor.read(cx).input.read(cx).focus_handle(cx),
            false => host.held.clone(),
        }
        .focus(window, cx);
        let reading = crate::ax::audit::observe(window, cx, "w", true, |_| true, snap);
        let focus: Vec<Vec<String>> = reading
            .snapshots
            .iter()
            .map(|nodes| {
                nodes
                    .iter()
                    .filter(|node| node.state.contains(&"focused"))
                    .map(|node| node.id.clone())
                    .collect()
            })
            .collect();
        (crate::ax::audit::audit(&reading, false), focus)
    });
    let text = native.update(|_, cx| host.read(cx).editor.read(cx).input.read(cx).value());
    (report, focus, text.to_string())
}

/// The door's Tab walk from inside the editor (docs/ax.md §1.1): Tab
/// indents and the focus stays, which the audit still reports (AX-022);
/// then the walk leaves by Esc, Tab, the way out Help gives, and reaches
/// the stop painted after the editor instead of indenting until it gives
/// up.
#[cfg(test)]
#[gpui_kit::test]
fn the_door_walk_leaves_the_editor_by_esc_then_tab(cx: &mut gpui_kit::TestAppContext) {
    let (report, focus, text) = door_walk(cx, true, false);
    let at = |step: usize| focus[step].join(",");
    assert_eq!(at(0), "w:editor-field", "the walk starts in the editor");
    assert!(
        focus.iter().any(|ids| ids == &["w:after"]),
        "the walk reaches the stop after the editor: {focus:?}"
    );
    let stays: Vec<&str> = report
        .violations
        .iter()
        .filter(|violation| violation.rule == "AX-022")
        .map(|violation| violation.id.as_str())
        .collect();
    assert_eq!(stays, ["step 1"], "the Tab the editor kept: {focus:?}");
    assert_eq!(report.presses, 3, "{focus:?}");
    assert_eq!(text, "one  ", "one Tab indented");
}

/// The first Tab lands on the editor and the next stays there: that stay
/// is not the walk coming back round, so once Esc, Tab has left the editor
/// the walk goes on past the stops the first snapshot shows (the stop below
/// the window's edge) until Tab brings it back to the editor.
#[cfg(test)]
#[gpui_kit::test]
fn the_door_walk_goes_round_after_leaving_the_editor_it_first_reached(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (report, focus, text) = door_walk(cx, false, true);
    let at = |step: usize| focus[step].join(",");
    assert_eq!(at(1), "w:editor-field", "the first Tab reaches the editor");
    assert_eq!(at(2), "w:editor-field", "the editor keeps the next");
    assert_eq!(at(3), "w:after", "Esc, Tab leaves it: {focus:?}");
    // on to the stop below the edge, which has no node, then round
    assert_eq!(report.presses, 5, "{focus:?}");
    assert_eq!(at(5), "w:editor-field", "back round: {focus:?}");
    assert_eq!(text, "one  ", "one Tab indented");
}
