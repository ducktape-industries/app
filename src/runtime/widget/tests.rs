use super::present::native_root;
use super::*;

/// `EditorStore` keys every editor by the `AuthoredPath` walked from the
/// guest's OWN root (`guest/requests.rs`'s `tick`, never wrapped). The
/// native widget tree is built from `native_root(root)` instead — the
/// wrapper `native_root` adds around that same root so an unsized guest
/// root still fills the seat. If that wrapper carried an id, every
/// descendant's `AuthoredPath` as walked from the RENDERED tree would
/// carry one extra leading segment the store never indexed under, and a
/// native editor field could never find its `EditorStore` entry: it would
/// keep typing locally (GPUI's own default text handling on an
/// editable-by-default field) while the guest's document — and everything
/// gated on it, like a claimed Enter or the Send button — never moved.
/// Reproduces that class of bug directly against the two real tree walks,
/// with no gpui window needed.
#[test]
fn native_root_does_not_shift_the_authored_path_editor_store_indexes_by() {
    let editor = wire::Node::Editor {
        binding: None,
        id: wire::ElementIdWire::Name("editor".into()),
        style: Default::default(),
        placeholder: String::new(),
        label: None,
        document: wire::editor_document::EditorDocumentRef {
            document: "doc".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: wire::EditorCursor::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
    };
    let panel = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("panel".into())),
        style: Default::default(),
        interactivity: Default::default(),
        children: vec![editor],
    });

    // The guest's own root, unwrapped: what `EditorStore::replace` indexes,
    // exactly as `guest/requests.rs`'s `tick` calls it.
    let store = EditorStore::new(0);
    store
        .replace(&panel)
        .expect("a valid editor tree validates");

    // The tree the renderer actually walks to mount native widgets — the
    // one and only tree `native_root` ever produces for it.
    let rendered = native_root(panel);
    let mounted_key = editor_authored_path(&rendered).expect("the editor is still in the tree");

    assert!(
        store.projection(&mounted_key).is_some(),
        "a native editor's own mounted path must resolve in the EditorStore \
         the guest's unwrapped tree populated; native_root must add no identity"
    );
}

/// The `AuthoredPath` to the first `Editor` node in `root`, walked the same
/// way `crate::render`'s node lowering (and `EditorStore::collect`) do: by
/// `crate::render::enter_scope`, which is what decides whether a node
/// contributes a path segment at all.
fn editor_authored_path(root: &wire::Node) -> Option<crate::render::AuthoredPath> {
    fn walk(
        node: &wire::Node,
        path: &mut crate::render::AuthoredPath,
    ) -> Option<crate::render::AuthoredPath> {
        let entered = crate::render::enter_scope(node, path);
        let found = matches!(node, wire::Node::Editor { .. })
            .then(|| path.clone())
            .or_else(|| node.children().iter().find_map(|child| walk(child, path)));
        if entered {
            path.pop();
        }
        found
    }
    walk(root, &mut crate::render::AuthoredPath::new())
}

#[gpui_kit::test]
fn same_module_instances_receive_independent_props(cx: &mut gpui_kit::TestAppContext) {
    let first = cx.new(|_| NativeModuleView::new("independent-props-test"));
    let second = cx.new(|_| NativeModuleView::new("independent-props-test"));
    first.update(cx, |view, cx| view.set_props(b"channel-one".to_vec(), cx));
    second.update(cx, |view, cx| view.set_props(b"channel-two".to_vec(), cx));
    cx.update(|cx| {
        let first = first.read(cx);
        let second = second.read(cx);
        assert_ne!(first.instance, second.instance);
        assert!(!Arc::ptr_eq(&first.seat, &second.seat));
        assert_eq!(
            first.seat.lock().unwrap().props.as_deref(),
            Some(b"channel-one".as_slice())
        );
        assert_eq!(
            second.seat.lock().unwrap().props.as_deref(),
            Some(b"channel-two".as_slice())
        );
    });
    first.update(cx, |view, cx| view.set_props(b"channel-three".to_vec(), cx));
    cx.update(|cx| {
        assert_eq!(
            second.read(cx).seat.lock().unwrap().props.as_deref(),
            Some(b"channel-two".as_slice())
        );
    });
}

/// The root a view is drawn in is laid out from the view's own minimum.
#[test]
fn a_view_is_laid_out_from_its_own_minimum() {
    crate::runtime::seat_for_test("laid-out-from", 560);
    let seat = registry().lock().unwrap()[&("laid-out-from", 0)].clone();
    let Slot::Ready(guest) = &seat.lock().unwrap().slot else {
        panic!("seated");
    };
    assert_eq!(laid_out_from(guest), 560.);
}

/// The idle rule of docs/perf.md, on the counters `GET /perf` serves: a
/// seated view at rest is drawn by the window every frame, but its tree
/// renders again only for a tick that changed it, so `renders ≤ ticks + 2`.
/// The view draws #347's tree — a paragraph in id-less boxes in a named
/// one — under the selection layer whose sweep of a cached frame's
/// paragraphs refreshed the window on every frame before #347: renders far
/// above ticks is that loop, or any other that keeps a cached tree dirty.
/// On the cached path, no a11y reader.
#[gpui_kit::test]
fn an_idle_view_renders_no_more_than_it_ticks(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{IntoElement, ParentElement as _, Render, Styled as _, div, px, size};

    struct Seat(gpui_kit::Entity<NativeModuleView>);
    impl Render for Seat {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl IntoElement {
            // it sweeps the paragraphs a cached frame did not register
            div()
                .size_full()
                .child(gpui_kit::base::TextSelectionLayer)
                .child(self.0.clone())
        }
    }

    let bare = |children: Vec<wire::Node>| {
        wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: div().p_2().style().clone(),
            interactivity: Default::default(),
            children,
        })
    };
    let paragraph = wire::Node::RichText {
        id: Some(wire::ElementIdWire::Name("line".into())),
        style: div().h(px(20.)).style().clone(),
        text: "a line".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    };
    let card = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("card".into())),
        style: div().w(px(200.)).h(px(100.)).style().clone(),
        interactivity: Default::default(),
        children: vec![bare(vec![bare(vec![paragraph])])],
    });
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_drawing_for_test("idle-renders-test", 320, card);
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        Seat(cx.new(|_| NativeModuleView::new("idle-renders-test")))
    });
    let seat = window.root(cx).unwrap();
    let view = seat.read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    // the desk redraws its seats on every frame it draws
    for _ in 0..8 {
        native.update(|window, cx| {
            seat.update(cx, |_, cx| cx.notify());
            view.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    }
    let counted = &crate::perf::snapshot(false)["views"]["idle-renders-test"];
    let (ticks, renders) = (counted["ticks"].as_u64(), counted["renders"].as_u64());
    let (Some(ticks), Some(renders)) = (ticks, renders) else {
        panic!("the seat counts its ticks and its tree's renders: {counted}");
    };
    assert!(
        renders <= ticks + 2,
        "{renders} renders over {ticks} ticks: the tree is redrawn without a tick"
    );
}

/// A one-line field's caret blinks only while the field has the keys. gpui-base
/// 0.6.4 started the blink on the programmatic `set_value` a mount does and
/// never stopped it on a field nothing focused, so every view with a field
/// re-rendered twice a second at rest (the perf-breaches report's §1; upstream
/// gpui-kit #3138, fixed in 0.7.0 by #3139/#3140). With the window active: an
/// unfocused field renders its view 0 times over an idle 2 s, a focused one
/// renders once per caret toggle, and blurring it stops that again.
#[gpui_kit::test]
fn a_one_line_field_blinks_only_while_focused(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{IntoElement, ParentElement as _, Render, Styled as _, div, px, size};
    use std::time::Duration;

    struct Seat(gpui_kit::Entity<NativeModuleView>);
    impl Render for Seat {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }

    let field = wire::Node::Input {
        options: wire::InputOptions {
            label: "Filter members".into(),
            ..Default::default()
        },
        id: wire::ElementIdWire::Name("filter".into()),
        placeholder: "Filter by name".into(),
        value: "a value the mount sets".into(),
        on_input: Some(1),
        on_submit: None,
        secure: false,
        style: div().w(px(200.)).h(px(24.)).style().clone(),
    };
    let card = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("card".into())),
        style: div().w(px(240.)).h(px(100.)).style().clone(),
        interactivity: Default::default(),
        children: vec![field],
    });
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_drawing_for_test("blink-renders-test", 320, card);
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        Seat(cx.new(|_| NativeModuleView::new("blink-renders-test")))
    });
    let seat = window.root(cx).unwrap();
    let view = seat.read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let frame = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            seat.update(cx, |_, cx| cx.notify());
            view.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    };
    let renders = || {
        crate::perf::snapshot(false)["views"]["blink-renders-test"]["renders"]
            .as_u64()
            .unwrap_or(0)
    };
    // two seconds at rest, drawn every 100 ms as the desk draws its seats
    let idle = |native: &mut gpui_kit::VisualTestContext| {
        let before = renders();
        for _ in 0..20 {
            native.executor().advance_clock(Duration::from_millis(100));
            frame(native);
        }
        renders() - before
    };
    for _ in 0..4 {
        frame(&mut native);
    }
    // the caret only shows in an active window
    native.update(|window, _| window.activate_window());
    frame(&mut native);
    assert!(native.update(|window, _| window.is_window_active()));
    let unfocused = idle(&mut native);

    native.update(|window, cx| window.focus_next(cx));
    frame(&mut native);
    let input = native.update(|_, cx| {
        let content = view.read(cx).content.clone().expect("mounted");
        content.read(cx).first_input_for_test().expect("a field")
    });
    assert!(native.update(|window, cx| {
        gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window)
    }));
    let focused = idle(&mut native);

    native.update(|window, cx| window.blur(cx));
    frame(&mut native);
    let blurred = idle(&mut native);

    eprintln!("idle 2 s renders: unfocused {unfocused}, focused {focused}, blurred {blurred}");
    assert_eq!(unfocused, 0, "an unfocused field keeps an idle view still");
    assert!(
        focused >= 2,
        "a focused field's caret toggles: {focused} renders"
    );
    assert_eq!(blurred, 0, "blurring stops the blink");
}
