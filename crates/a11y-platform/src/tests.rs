//! The wrapper's own contract, and each accessibility rule the host keeps over published gpui
//! (design rows 3, 10, 12, 13, 15, 21, 22), read and acted on through the wrapper.
use std::{
    cell::Cell,
    hash::{BuildHasher, RandomState},
    rc::Rc,
};

use gpui::{
    AnyView, AppContext, Context, ElementId, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, StyleRefinement, Styled, Window, accesskit,
    deferred, div, prelude::FluentBuilder, px,
};

use accesskit::{Action, Node, NodeId, Role, TreeUpdate};

use crate::headless::Harness;

fn active<V: Render + 'static>(
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
) -> Harness<V> {
    let harness = Harness::open(build);
    assert!(
        harness.a11y.activate(),
        "gpui gave the window its callbacks"
    );
    harness.draw();
    harness
}

fn find<'a>(tree: &'a TreeUpdate, label: &str) -> (NodeId, &'a Node) {
    tree.nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label))
        .map(|(id, node)| (*id, node))
        .unwrap_or_else(|| panic!("no node labelled {label}"))
}

fn has(tree: &TreeUpdate, label: &str) -> bool {
    tree.nodes
        .iter()
        .any(|(_, node)| node.label() == Some(label))
}

struct SendButton {
    clicks: usize,
}

impl Render for SendButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("send")
                .role(Role::Button)
                .aria_label(format!("Send {}", self.clicks))
                .focusable()
                .w(px(80.))
                .h(px(20.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.clicks += 1;
                    cx.notify();
                })),
        )
    }
}

#[test]
fn activates_reads_and_acts_on_the_tree_through_the_wrapper() {
    let h = Harness::open(|_, _| SendButton { clicks: 0 });
    assert!(h.a11y.tree().is_none(), "no tree before activation");

    assert!(h.a11y.activate());
    h.draw();
    let send = find(&h.tree(), "Send 0").0;

    h.act(Action::Click, send);
    assert_eq!(h.with(|send, _, _| send.clicks), 1);
    assert!(has(&h.tree(), "Send 1"), "the tree follows the click");
}

#[test]
fn hands_the_callbacks_and_every_tree_on_to_the_wrapped_window() {
    let h = Harness::open(|_, _| SendButton { clicks: 0 });
    assert!(
        h.inner.adapter_activates(),
        "the wrapped window got callbacks"
    );
    h.draw();
    let wrapped = h.inner.tree().expect("the wrapped window got a tree");
    assert!(has(&wrapped, "Send 0"));
    assert!(
        h.a11y.tree() == Some(wrapped),
        "the wrapper keeps the tree it handed on"
    );
}

struct Disabled;

impl Render for Disabled {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("save")
                .role(Role::Button)
                .aria_label("Save")
                .w(px(40.))
                .h(px(20.))
                .a11y_synthetic_children(|children| children.parent_node().set_disabled()),
        )
    }
}

#[test]
fn row_3_disabled_through_synthetic_children() {
    let h = active(|_, _| Disabled);
    assert!(find(&h.tree(), "Save").1.is_disabled());
}

struct Stops {
    before: FocusHandle,
    field: FocusHandle,
    after: FocusHandle,
}

impl Render for Stops {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let stop = |id: &'static str, handle: &FocusHandle| {
            div().id(id).track_focus(handle).w(px(10.)).h(px(10.))
        };
        div()
            .size_full()
            .child(stop("before", &self.before))
            .child(stop("field", &self.field).child(div().id("input").w(px(5.)).h(px(5.))))
            .child(stop("after", &self.after))
    }
}

#[test]
fn row_10_one_tab_stop_per_focus_handle() {
    let h = active(|_, cx| Stops {
        before: cx.focus_handle().tab_stop(true),
        field: cx.focus_handle().tab_stop(true),
        after: cx.focus_handle().tab_stop(true),
    });
    h.with(|stops, window, cx| window.focus(&stops.field, cx));
    h.draw();
    h.with(|_, window, cx| window.focus_prev(cx));
    h.draw();
    assert!(
        h.with(|stops, window, _| stops.before.is_focused(window)),
        "Shift+Tab leaves the field"
    );
    h.with(|stops, window, cx| window.focus(&stops.field, cx));
    h.draw();
    h.with(|_, window, cx| window.focus_next(cx));
    h.draw();
    assert!(
        h.with(|stops, window, _| stops.after.is_focused(window)),
        "Tab leaves the field"
    );
}

struct Composites {
    pane: FocusHandle,
    left: FocusHandle,
    right: FocusHandle,
}

fn composite(
    id: &'static str,
    row: &'static str,
    label: &'static str,
    handle: &FocusHandle,
    window: &Window,
) -> impl IntoElement {
    let claims = handle.is_focused(window);
    div()
        .id(id)
        .role(Role::ListBox)
        .aria_label(label)
        .track_focus(handle)
        .w(px(50.))
        .h(px(20.))
        .child(
            div()
                .id(row)
                .role(Role::ListBoxOption)
                .aria_label(row)
                .aria_selected(true)
                .w(px(50.))
                .h(px(10.))
                .when(claims, |row| row.aria_active_descendant()),
        )
}

impl Render for Composites {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("pane")
            .role(Role::Group)
            .aria_label("pane")
            .track_focus(&self.pane)
            .size_full()
            .child(composite(
                "left",
                "left row",
                "left list",
                &self.left,
                window,
            ))
            .child(composite(
                "right",
                "right row",
                "right list",
                &self.right,
                window,
            ))
    }
}

#[test]
fn row_12_a_claim_counts_only_under_its_own_focused_composite() {
    let h = active(|_, cx| Composites {
        pane: cx.focus_handle(),
        left: cx.focus_handle(),
        right: cx.focus_handle(),
    });
    h.with(|composites, window, cx| window.focus(&composites.pane, cx));
    h.draw();
    let tree = h.tree();
    assert_eq!(
        tree.focus,
        find(&tree, "pane").0,
        "a focused box over two composites"
    );
    h.with(|composites, window, cx| window.focus(&composites.right, cx));
    h.draw();
    let tree = h.tree();
    assert_eq!(
        tree.focus,
        find(&tree, "right row").0,
        "the focused composite's claim"
    );
}

struct Card(Rc<Cell<usize>>);

impl Render for Card {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.set(self.0.get() + 1);
        div()
            .id("card")
            .role(Role::Group)
            .aria_label("Card")
            .size_full()
    }
}

struct Desk {
    card: Entity<Card>,
}

impl Render for Desk {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let view = AnyView::from(self.card.clone());
        let pane = if window.is_a11y_active() {
            view.into_any_element()
        } else {
            view.cached(StyleRefinement::default().size_full())
                .into_any_element()
        };
        div().size_full().child(pane)
    }
}

#[test]
fn row_13_a_pane_drawn_uncached_while_at_is_active_keeps_its_nodes() {
    let renders = Rc::new(Cell::new(0));
    let counted = renders.clone();
    let h = active(move |_, cx| Desk {
        card: cx.new(|_| Card(counted)),
    });
    let before = renders.get();
    let mut present = Vec::new();
    for _ in 0..3 {
        h.with(|_, _, cx| cx.notify());
        h.draw_unforced();
        present.push(has(&h.tree(), "Card"));
    }
    assert_eq!(present, [true, true, true]);
    assert_eq!(renders.get() - before, 3, "the pane renders on every frame");
}

struct Covered {
    clicks: Rc<Cell<usize>>,
}

impl Render for Covered {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let on_click = self.clicks.clone();
        let on_action = self.clicks.clone();
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("go")
                    .role(Role::Button)
                    .aria_label("Go")
                    .focusable()
                    .w(px(80.))
                    .h(px(20.))
                    .on_click(move |_, _, _| on_click.set(on_click.get() + 1))
                    .on_a11y_action(Action::Click, move |_, _, _| {
                        on_action.set(on_action.get() + 1)
                    }),
            )
            .child(
                div()
                    .id("cover")
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(200.))
                    .h(px(200.))
                    .occlude(),
            )
    }
}

#[test]
fn row_15_an_at_click_on_a_covered_control_runs_the_host_rule_once() {
    let clicks = Rc::new(Cell::new(0));
    let shared = clicks.clone();
    let h = active(move |_, _| Covered { clicks: shared });
    let go = find(&h.tree(), "Go").0;
    h.act(Action::Click, go);
    assert_eq!(clicks.get(), 1);
}

struct Dialog;

impl Render for Dialog {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            deferred(
                div()
                    .id("dialog")
                    .role(Role::Dialog)
                    .aria_label("Confirm")
                    .w(px(100.))
                    .h(px(50.))
                    .child(
                        div()
                            .id("ok")
                            .role(Role::Button)
                            .aria_label("OK")
                            .w(px(40.))
                            .h(px(20.)),
                    ),
            )
            .with_priority(1),
        )
    }
}

#[test]
fn row_21_a_dialogs_controls_sit_under_its_node() {
    let h = active(|_, _| Dialog);
    let tree = h.tree();
    let ok = find(&tree, "OK").0;
    assert!(find(&tree, "Confirm").1.children().contains(&ok));
}

fn keyed_id(secret: &RandomState, program: &str, path: &[&str]) -> ElementId {
    ElementId::from(secret.hash_one((program, path)) as usize)
}

struct Ids {
    secret: RandomState,
}

impl Render for Ids {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                div()
                    .id("send")
                    .role(Role::Button)
                    .aria_label("host send")
                    .accessibility_id("console:send")
                    .w(px(40.))
                    .h(px(20.)),
            )
            .child(
                div()
                    .id(keyed_id(&self.secret, "chat", &["send"]))
                    .role(Role::Button)
                    .aria_label("view send")
                    .accessibility_id("chat/send")
                    .w(px(40.))
                    .h(px(20.)),
            )
    }
}

#[test]
fn row_22_a_keyed_view_id_never_meets_a_host_node() {
    let h = active(|_, _| Ids {
        secret: RandomState::new(),
    });
    let tree = h.tree();
    let (host, host_node) = find(&tree, "host send");
    let (view, view_node) = find(&tree, "view send");
    assert_ne!(host, view);
    assert_eq!(host_node.author_id(), Some("console:send"));
    assert_eq!(view_node.author_id(), Some("chat/send"));
    let (one, two) = (RandomState::new(), RandomState::new());
    assert_ne!(
        keyed_id(&one, "chat", &["send"]),
        keyed_id(&two, "chat", &["send"])
    );
}
