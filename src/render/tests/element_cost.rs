//! A host element costs what its node sets (briefs/perf-root, H2). gpui
//! keeps element state under an element's id and reads it in each of its
//! three passes, so the host gives a container an id only when something
//! reads one, and draws the canvas that brings a scroller to a node only on
//! a node that claims.
use super::*;

/// A box the view gave no id, around a text `leaf`.
fn around(
    leaf: &str,
    style: gpui_kit::StyleRefinement,
    interactivity: Option<wire::Interactivity>,
) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: crate::render::test_style(style),
        interactivity: interactivity.map(Box::new),
        children: vec![text(leaf, leaf)],
    })
}

/// The id gpui files the text `leaf` under, innermost last: its own, and
/// before it those of the elements around it that have one.
fn filed(native: &mut gpui_kit::VisualTestContext, leaf: &'static str) -> Vec<ElementId> {
    native.update(|window, _| window.find(leaf).path().to_vec())
}

/// Whether `id` is the id the host makes for a container the view gave none.
fn host_container(id: &ElementId) -> bool {
    crate::render::is_host_id(id)
        && matches!(id, ElementId::NamedChild(_, name) if name.starts_with("container-"))
}

/// A container that sets nothing gpui keeps state for is a `Div` with no
/// id: what is under it is filed straight under the nearest node that has
/// one, so gpui makes no global id for it and reads no element state for it
/// in any pass. A clip is a style, and needs none either.
#[gpui_kit::test]
fn a_container_that_sets_nothing_has_no_id(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let plain = around("plain", div().p_2().style().clone(), None);
    let clipped = around("clipped", div().overflow_hidden().style().clone(), None);
    let nested = wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: crate::render::plain_style(),
        interactivity: None,
        children: vec![around("nested", div().flex().style().clone(), None)],
    });
    let root = container("card", [plain, clipped, nested]);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    for leaf in ["plain", "clipped", "nested"] {
        let path = filed(&mut native, leaf);
        assert!(
            path.ends_with(&["card".into(), leaf.into()]),
            "{leaf} is filed straight under the card: {path:?}"
        );
        assert!(!path.iter().any(host_container), "{leaf}: {path:?}");
    }
}

/// Each thing gpui or the host reads under a container's id still finds
/// one: the id the view gave; a scroller's offset, on either axis; and any
/// interactivity (a listener, focus, a role, a tooltip, a hover, active or
/// group style, a claim).
#[gpui_kit::test]
fn a_container_something_reads_keeps_its_id(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let plain = || div().p_1().style().clone();
    let scrolls = |x: bool| {
        let mut style = div().size(px(20.)).style().clone();
        match x {
            true => style.overflow.x = Some(gpui_kit::Overflow::Scroll),
            false => style.overflow.y = Some(gpui_kit::Overflow::Scroll),
        }
        style
    };
    let lit = || {
        Some(crate::render::test_style(
            div().bg(rgb(0x303846)).style().clone(),
        ))
    };
    let with =
        |leaf: &str, interactivity: wire::Interactivity| around(leaf, plain(), Some(interactivity));
    let none = wire::Interactivity::default;
    let kept = [
        around("scrolls-y", scrolls(false), None),
        around("scrolls-x", scrolls(true), None),
        with(
            "click",
            wire::Interactivity {
                on_click: Some(1),
                ..none()
            },
        ),
        with(
            "pointer",
            wire::Interactivity {
                on_mouse_move: Some(2),
                ..none()
            },
        ),
        with(
            "key",
            wire::Interactivity {
                on_key_down: Some(3),
                ..none()
            },
        ),
        with(
            "focus",
            wire::Interactivity {
                focusable: true,
                ..none()
            },
        ),
        with(
            "role",
            wire::Interactivity {
                role: Some(gpui_kit::Role::Group),
                ..none()
            },
        ),
        with(
            "label",
            wire::Interactivity {
                aria: wire::Aria {
                    label: Some("Named".into()),
                    ..Default::default()
                },
                ..none()
            },
        ),
        with(
            "tooltip",
            wire::Interactivity {
                tooltip: Some(wire::Tooltip {
                    request: 4,
                    hoverable: false,
                    delay_ms: 10,
                }),
                ..none()
            },
        ),
        with(
            "hover",
            wire::Interactivity {
                hover: lit(),
                ..none()
            },
        ),
        with(
            "active",
            wire::Interactivity {
                active: lit(),
                ..none()
            },
        ),
        with(
            "group",
            wire::Interactivity {
                group: Some("row".into()),
                ..none()
            },
        ),
        with(
            "claim",
            wire::Interactivity {
                role: Some(gpui_kit::Role::ListBoxOption),
                aria: wire::Aria {
                    active_descendant: true,
                    ..Default::default()
                },
                ..none()
            },
        ),
    ];
    let leaves = [
        "scrolls-y",
        "scrolls-x",
        "click",
        "pointer",
        "key",
        "focus",
        "role",
        "label",
        "tooltip",
        "hover",
        "active",
        "group",
        "claim",
    ];
    let authored = container("authored", [text("under-authored", "x")]);
    let root = container("card", kept.into_iter().chain([authored]));
    let window = cx.open_window(size(px(400.), px(600.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let mut ids = std::collections::HashSet::new();
    for leaf in leaves {
        let path = filed(&mut native, leaf);
        let [.., card, own, _] = &path[..] else {
            panic!("{leaf}: {path:?}")
        };
        assert_eq!(card, &ElementId::from("card"), "{leaf}: {path:?}");
        assert!(
            host_container(own),
            "{leaf} has an id of the host's: {path:?}"
        );
        assert!(ids.insert(own.clone()), "{leaf} shares {own:?}");
    }
    let path = filed(&mut native, "under-authored");
    assert!(
        path.ends_with(&["card".into(), "authored".into(), "under-authored".into()]),
        "{path:?}"
    );
}

/// A scroller the view gave no id keeps its offset in gpui's element
/// state, under the id the host gives it: the wheel moves it, and the next
/// frame of the same tree draws it where the wheel left it. With no id it
/// would have no offset to move.
#[gpui_kit::test]
fn a_scroller_the_view_gave_no_id_keeps_its_offset(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let frame = || {
        let rows = (0..10).map(|n| {
            container_with_style(
                &format!("row-{n}"),
                div().h(px(40.)).flex_shrink_0().style().clone(),
                [],
            )
        });
        let mut style = div()
            .flex()
            .flex_col()
            .w(px(200.))
            .h(px(100.))
            .style()
            .clone();
        style.overflow.y = Some(gpui_kit::Overflow::Scroll);
        let scroller = wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: crate::render::test_style(style),
            interactivity: None,
            children: rows.collect(),
        });
        container("card", [scroller])
    };
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(frame()));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let top = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find("row-0").bounds().top())
        })
    };
    assert_eq!(top(&mut native), 0.);
    native.update(|window, cx| {
        window.render_frame(cx);
        let wheel = gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-120.)));
        window.scroll("row-1", wheel, cx);
    });
    assert_eq!(top(&mut native), -120., "the wheel scrolls it");
    tree.update(&mut native, |tree, cx| tree.replace(frame(), &[], cx));
    assert_eq!(top(&mut native), -120., "the next frame keeps the offset");
    assert_eq!(top(&mut native), -120., "and the one after");
}

/// A card of three nodes the view gave no id: a box 10 px tall, with a hover
/// style when `lit`; a scroller of ten rows; a text.
fn anonymous(lit: bool) -> wire::Node {
    let hover = lit.then(|| wire::Interactivity {
        hover: Some(crate::render::test_style(
            div().bg(rgb(0x303846)).style().clone(),
        )),
        ..Default::default()
    });
    let plain = wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: crate::render::test_style(div().h(px(10.)).flex_shrink_0().style().clone()),
        interactivity: hover.map(Box::new),
        children: Vec::new(),
    });
    let rows = (0..10).map(|n| {
        container_with_style(
            &format!("row-{n}"),
            div().h(px(40.)).flex_shrink_0().style().clone(),
            [],
        )
    });
    let mut style = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .w(px(200.))
        .h(px(100.))
        .style()
        .clone();
    style.overflow.y = Some(gpui_kit::Overflow::Scroll);
    let scroller = wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: crate::render::test_style(style),
        interactivity: None,
        children: rows.collect(),
    });
    let words = wire::Node::Text(view_wire::TextNode {
        id: None,
        style: crate::render::test_style(gpui_kit::StyleRefinement::default()),
        content: "words".into(),
    });
    container_with_style(
        "card",
        div().flex().flex_col().style().clone(),
        [plain, scroller, words],
    )
}

/// A host number is a node's place among the nodes the view gave no id,
/// whatever each of them sets: a box that starts to take an id (here it
/// gains a hover style) moves no number after it. So the scroller after it
/// is the same element on the next frame, and gpui hands it the offset the
/// wheel left under its id.
#[gpui_kit::test]
fn a_scroller_keeps_its_offset_when_a_box_before_it_takes_an_id(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(300.)), |_, _| {
        ViewTree::new(anonymous(false))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let top = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find("row-0").bounds().top())
        })
    };
    assert_eq!(top(&mut native), 10.);
    native.update(|window, cx| {
        let wheel = gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-120.)));
        window.scroll("row-1", wheel, cx);
    });
    assert_eq!(top(&mut native), -110., "the wheel scrolls it");
    tree.update(&mut native, |tree, cx| {
        tree.replace(anonymous(true), &[], cx)
    });
    assert_eq!(
        top(&mut native),
        -110.,
        "the box before it gained a hover style: the scroller keeps its offset"
    );
}

/// The same for a text the view gave no id: its number, so its id, so the
/// accessibility node and whatever gpui keeps under it, stays when a box
/// before it starts to take an id.
#[gpui_kit::test]
fn a_text_keeps_its_number_when_a_box_before_it_takes_an_id(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(300.)), |_, _| {
        ViewTree::new(anonymous(false))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    // the number of the one text the host numbered
    let number = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            window.render_frame(cx);
            (0..8).find(|n| {
                let id = crate::render::host_id(format!("text-{n}"));
                window.try_find(id).is_some()
            })
        })
    };
    let before = number(&mut native);
    assert!(before.is_some(), "the text has a host number");
    tree.update(&mut native, |tree, cx| {
        tree.replace(anonymous(true), &[], cx)
    });
    assert_eq!(
        number(&mut native),
        before,
        "the box before it gained a hover style: the text keeps its number"
    );
}

/// A scroller of ten rows, each with an id, the first `first` px tall;
/// `claim` is the row that claims.
fn rows(claim: Option<usize>, first: f32) -> wire::Node {
    let rows = (0..10).map(|n| {
        let height = if n == 0 { first } else { 40. };
        let mut row = container_with_style(
            &format!("row-{n}"),
            div().h(px(height)).flex_shrink_0().style().clone(),
            [text(&format!("label-{n}"), "row")],
        );
        if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut row {
            let interactivity = interactivity.get_or_insert_default();
            interactivity.role = Some(gpui_kit::Role::ListBoxOption);
            interactivity.aria.active_descendant = claim == Some(n);
        }
        row
    });
    let mut style = div()
        .flex()
        .flex_col()
        .w(px(200.))
        .h(px(100.))
        .style()
        .clone();
    style.overflow.y = Some(gpui_kit::Overflow::Scroll);
    container_with_style("list", style, rows)
}

/// A node that does not claim has no canvas over it: nothing measures it,
/// so a frame that moves every row is drawn once. The canvas is on the row
/// that claims, and its one move of the scroller asks for the one render
/// that draws the row there; a claim that stays asks for nothing.
#[gpui_kit::test]
fn only_a_claim_asks_for_a_second_render(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, cx| {
        Seat(cx.new(|_| ViewTree::new(rows(None, 40.))))
    });
    let seat = window.root(cx).unwrap();
    let tree = seat.read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    // the renders `frame` costs, over five draws of the desk, and where
    // they leave the scroller
    let mut renders = |frame: Option<wire::Node>| {
        let before = tree.read_with(&native, |tree, _| tree.renders);
        if let Some(frame) = frame {
            tree.update(&mut native, |tree, cx| tree.replace(frame, &[], cx));
        }
        for _ in 0..5 {
            native.update(|window, cx| {
                // the desk draws its seats again: `render_frame` would refresh
                seat.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            });
            native.run_until_parked();
        }
        tree.read_with(&native, |tree, _| {
            let offset = tree.scrolls[&vec![named_id("list")]].offset().y;
            (tree.renders - before, f32::from(offset))
        })
    };
    assert_eq!(
        renders(None),
        (0, 0.),
        "at rest the open tree is not drawn again"
    );
    assert_eq!(
        renders(Some(rows(None, 60.))),
        (1, 0.),
        "every row below the first moved, and no row claims: one render"
    );
    assert_eq!(
        renders(Some(rows(Some(7), 60.))),
        (2, -240.),
        "row 7 (300..340) claims below the fold: its frame, and the one scrolled to it"
    );
    assert_eq!(
        renders(Some(rows(Some(7), 40.))),
        (1, -240.),
        "the claim stays where the rows move: one render, and the offset is left alone"
    );
}
