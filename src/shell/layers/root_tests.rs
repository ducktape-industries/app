//! The window's layers, counted: each draws for what it observes and for
//! nothing else (P1, P6 in the app), and the root lays them out as the
//! window's kind and screen say (docs/perf.md).
use super::BAR;
use super::tests::{
    Seed, frame, line, open_console, pane, polled, pop_out, set_motion, set_screen, toast,
    tree_renders,
};
use crate::shell::PaneMessage;
use crate::shell::entities::tests::{source, status};
use crate::shell::entities::{Overlay, Popover, Screen};
use crate::shell::panes_tests::{console, draw, ids, settle, window_count};
use gpui_kit::{TestAppContext, VisualTestContext};

#[gpui_kit::test]
fn a_pulse_re_renders_the_dot_and_not_the_chrome(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, _, mut native) = console(cx);
    native.update(|_, cx| cx.set_reduce_motion(false));
    native.update(|window, _| window.activate_window());
    set_motion(&app, true, &mut native);
    polled(&app, status(7), &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    let count = |native: &mut VisualTestContext, stage: &str| {
        native.run_until_parked();
        window_count(key, stage)
    };
    let (chrome, dot, root) = (
        count(&mut native, "renders.chrome"),
        count(&mut native, "renders.dot"),
        count(&mut native, "renders"),
    );
    assert!(chrome > 0 && dot > 0, "the bar and the dot drew");
    for _ in 0..10 {
        ducktape_host::frames::deliver(&mut native, std::time::Duration::from_secs(1) / 30);
    }
    let pulsed = count(&mut native, "renders.dot") - dot;
    assert_eq!(
        pulsed, 10,
        "the dot pulsed {pulsed} times in ten pulses' frames"
    );
    assert_eq!(
        count(&mut native, "renders") - root,
        pulsed,
        "the root drew other than once per pulse"
    );
    assert_eq!(
        count(&mut native, "renders.chrome"),
        chrome,
        "a pulse drew the bar"
    );
}

fn render_counts(key: crate::runtime::WindowKey) -> Vec<(String, u64)> {
    let snapshot = crate::perf::snapshot(false);
    let counts = snapshot["windows"][key.0.to_string()]
        .as_object()
        .expect("the window counts");
    counts
        .iter()
        .filter(|(name, _)| name.starts_with("renders"))
        .map(|(name, count)| (name.clone(), count.as_u64().unwrap_or(0)))
        .collect()
}

#[gpui_kit::test]
fn a_still_chain_draws_no_frame(cx: &mut TestAppContext) {
    use crate::shell::entities::SessionState;
    const PROGRAM: &str = "root-still-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    let (source, _) = source(|_| Ok(status(7)));
    let (fed, heads) = ducktape_host::session::tests::heads();
    let chain = app
        .session
        .read_with(&native, |session, _| session.parts().0);
    chain.update(&mut native, |chain, _| chain.height = 7);
    app.session.update(&mut native, |session, cx| {
        let on_node = SessionState {
            connected: true,
            network: "testkit".into(),
            chain: "testkit".into(),
            ..SessionState::booted()
        };
        session.heads_through(fed);
        session.seed_connected(on_node, source, cx)
    });
    let poll = |native: &mut VisualTestContext| {
        heads.send(7);
        native.run_until_parked();
    };
    poll(&mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    let still = render_counts(key);
    let tree = tree_renders(PROGRAM);
    assert!(
        still
            .iter()
            .any(|(name, count)| name == "renders.chrome" && *count > 0),
        "the bar never drew: {still:?}"
    );
    for _ in 0..6 {
        poll(&mut native);
    }
    assert_eq!(
        render_counts(key),
        still,
        "a head that moved nothing drew the window"
    );
    assert_eq!(
        tree_renders(PROGRAM),
        tree,
        "a still chain drew a pane's tree"
    );
    heads.send(8);
    native.run_until_parked();
    let height = chain.read_with(&native, |chain, _| chain.height);
    assert_eq!(height, 8, "the heads were not followed");
    assert!(
        crate::perf::snapshot(false)["shell"]
            .get("dispatch")
            .is_none(),
        "something still times a reducer dispatch"
    );
}

#[gpui_kit::test]
fn a_pane_wake_leaves_the_chrome_alone(cx: &mut TestAppContext) {
    const PROGRAM: &str = "root-wake-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("first"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    for _ in 0..8 {
        frame(&mut native);
    }
    let (chrome, tree) = (window_count(key, "renders.chrome"), tree_renders(PROGRAM));
    assert!(chrome > 0 && tree > 0, "the bar and the tree drew");
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("again"));
    for _ in 0..4 {
        frame(&mut native);
    }
    assert_eq!(
        tree_renders(PROGRAM),
        tree + 1,
        "the woken tree drew other than once"
    );
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome,
        "a pane's wake drew the bar"
    );
}

#[gpui_kit::test]
fn a_toast_re_renders_its_layer_only(cx: &mut TestAppContext) {
    const PROGRAM: &str = "root-toast-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    for _ in 0..8 {
        frame(&mut native);
    }
    let (chrome, shown, tree) = (
        window_count(key, "renders.chrome"),
        window_count(key, "renders.toast"),
        tree_renders(PROGRAM),
    );
    assert!(chrome > 0 && shown > 0 && tree > 0, "every layer drew");
    toast(&app, "Saved", &mut native);
    frame(&mut native);
    assert_eq!(
        window_count(key, "renders.toast"),
        shown + 1,
        "the toast drew its layer other than once"
    );
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome,
        "a toast drew the bar"
    );
    assert_eq!(tree_renders(PROGRAM), tree, "a toast drew a pane's tree");
    let nodes = native.update(draw);
    assert!(
        ids(&nodes).iter().any(|id| id == "console:toast"),
        "no toast up: {nodes}"
    );
}

fn node_of(window: &gpui_kit::Window, name: &str) -> gpui_kit::accesskit::NodeId {
    let tree = window.a11y_tree().expect("the window built a tree");
    tree.nodes
        .iter()
        .find_map(|(node, _)| {
            let own = window.a11y_element_id(*node).and_then(|path| path.last());
            matches!(own, Some(gpui_kit::ElementId::Name(own)) if own.as_ref() == name)
                .then_some(*node)
        })
        .unwrap_or_else(|| panic!("no {name} node in the tree"))
}

fn placed(window: &gpui_kit::Window, names: &[&str]) -> Vec<(String, Option<String>)> {
    let tree = window.a11y_tree().expect("the window built a tree");
    let parents: std::collections::HashMap<_, _> = tree
        .nodes
        .iter()
        .flat_map(|(id, node)| node.children().iter().map(move |child| (*child, *id)))
        .collect();
    names
        .iter()
        .map(|name| {
            let parent = parents
                .get(&node_of(window, name))
                .and_then(|parent| window.a11y_element_id(*parent))
                .map(ToString::to_string);
            (name.to_string(), parent)
        })
        .collect()
}

fn door_ids(native: &mut VisualTestContext) -> Vec<String> {
    native.update(|window, _| {
        crate::ax::snapshot("console", window, false)
            .into_iter()
            .map(|node| node.id)
            .collect()
    })
}

#[gpui_kit::test]
fn under_a11y_every_layer_draws_uncached_and_keeps_its_nodes(cx: &mut TestAppContext) {
    use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
    const PROGRAM: &str = "root-a11y-view";
    const TAB: &str = "rail/root-a11y-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("a line"));
    app.rail.update(&mut native, |rail, cx| {
        rail.read_off(crate::runtime::Roster::listing(&[PROGRAM]), cx)
    });
    native.update(|_, cx| cx.set_reduce_motion(false));
    native.update(|window, _| window.activate_window());
    set_motion(&app, true, &mut native);
    polled(&app, status(7), &mut native);
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    native.update(|window, _| window.activate_a11y());
    for _ in 0..4 {
        frame(&mut native);
    }
    let names = [
        "menubar",
        "rail-rows",
        TAB,
        "pane/0/split",
        "pane/0/close",
        "pane/0/view",
        "rich-text",
    ];
    let (ids, first) = (
        door_ids(&mut native),
        native.update(|window, _| placed(window, &names)),
    );
    let draws = || {
        crate::perf::snapshot(false)["views"][PROGRAM]["draws"]
            .as_u64()
            .expect("the pane counts its draws")
    };
    let (chrome, strip, tree, drawn) = (
        window_count(key, "renders.chrome"),
        window_count(key, "renders.strip.0"),
        tree_renders(PROGRAM),
        draws(),
    );
    assert!(chrome > 0 && strip > 0 && tree > 0, "every layer drew");
    let pulse_and_read = |native: &mut VisualTestContext| {
        ducktape_host::frames::deliver(native, std::time::Duration::from_secs(1) / 30);
        native.update(|window, cx| window.draw(cx).clear(cx));
    };
    for pulse in 0..10 {
        pulse_and_read(&mut native);
        assert_eq!(door_ids(&mut native), ids, "after pulse {pulse}");
        assert_eq!(
            native.update(|window, _| placed(window, &names)),
            first,
            "after pulse {pulse} a node moved"
        );
    }
    let rose = |now: u64, before: u64| now.saturating_sub(before);
    let (chrome, strip, tree) = (
        rose(window_count(key, "renders.chrome"), chrome),
        rose(window_count(key, "renders.strip.0"), strip),
        rose(tree_renders(PROGRAM), tree),
    );
    assert!(
        chrome >= 10 && strip >= 10 && tree >= 10,
        "under AT every drawn frame renders each layer: bar {chrome}, strip {strip}, tree {tree} in ten pulses and ten reads"
    );
    assert!(
        draws() >= drawn + 20,
        "the pane view drew {} times in ten pulses and ten reads",
        draws() - drawn
    );
    native.update(|window, cx| {
        let target_node = node_of(window, TAB);
        window.dispatch_a11y_action(
            ActionRequest {
                action: Action::Focus,
                target_tree: TreeId::ROOT,
                target_node,
                data: None,
            },
            cx,
        );
    });
    frame(&mut native);
    let focused: Vec<_> = native.update(|window, _| {
        crate::ax::snapshot("console", window, false)
            .into_iter()
            .filter(|node| node.state.contains(&"focused"))
            .map(|node| node.id)
            .collect()
    });
    assert_eq!(focused, [format!("console:{TAB}")], "the Focus missed");
    super::tests::show(&view, Some(Overlay::Network), &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    let (ids, chrome) = (door_ids(&mut native), window_count(key, "renders.chrome"));
    let first = native.update(|window, _| placed(window, &["network-menu"]));
    for pulse in 0..5 {
        pulse_and_read(&mut native);
        assert_eq!(door_ids(&mut native), ids, "menu open, after pulse {pulse}");
        assert_eq!(
            native.update(|window, _| placed(window, &["network-menu"])),
            first
        );
    }
    assert!(
        window_count(key, "renders.chrome") >= chrome + 5,
        "under AT a pulse renders the bar under its menu"
    );
}

#[gpui_kit::test]
fn typing_in_spotlight_renders_the_overlay_layer_only(cx: &mut TestAppContext) {
    const PROGRAM: &str = "root-keystroke-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    super::tests::show(&view, Some(Overlay::Spotlight), &mut native);
    native.simulate_input("a");
    for _ in 0..8 {
        frame(&mut native);
    }
    let counts = || {
        (
            window_count(key, "renders.chrome"),
            window_count(key, "renders.toast"),
            tree_renders(PROGRAM),
        )
    };
    let (still, overlays) = (counts(), window_count(key, "renders.overlays"));
    assert!(
        still.0 > 0 && still.1 > 0 && still.2 > 0 && overlays > 0,
        "every layer drew"
    );
    native.simulate_input("l");
    frame(&mut native);
    let spotlight = app.windows.read_with(&native, |windows, _| {
        windows.own(key).unwrap().spotlight.clone()
    });
    let query = native.update(|_, cx| spotlight.read(cx).get().query.clone());
    assert_eq!(query, "al", "the keystrokes never reached the slice");
    assert!(
        window_count(key, "renders.overlays") > overlays,
        "the keystrokes drew no overlay layer"
    );
    assert_eq!(
        counts(),
        still,
        "a Spotlight keystroke drew the bar, the footer or a pane's tree"
    );
}

#[gpui_kit::test]
fn under_a11y_a_focus_move_re_renders_every_layer(cx: &mut TestAppContext) {
    use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
    const PROGRAM: &str = "root-focus-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    native.update(|window, _| {
        window.activate_window();
        window.activate_a11y();
    });
    for _ in 0..4 {
        frame(&mut native);
    }
    let focused = |native: &mut VisualTestContext| {
        native.update(|window, _| {
            crate::ax::snapshot("console", window, false)
                .into_iter()
                .find(|node| node.state.contains(&"focused"))
                .map(|node| node.id)
        })
    };
    native.update(|window, cx| {
        let target_node = node_of(window, "settings");
        window.dispatch_a11y_action(
            ActionRequest {
                action: Action::Focus,
                target_tree: TreeId::ROOT,
                target_node,
                data: None,
            },
            cx,
        );
    });
    native.simulate_keystrokes("shift-tab");
    native.simulate_keystrokes("tab");
    for _ in 0..4 {
        frame(&mut native);
    }
    assert_eq!(focused(&mut native).as_deref(), Some("console:settings"));
    let counts = || {
        [
            "renders",
            "renders.pane.0",
            "renders.chrome",
            "renders.strip.0",
            "renders.overlays",
            "renders.toast",
        ]
        .map(|name| window_count(key, name))
    };
    let (before, tree) = (counts(), tree_renders(PROGRAM));
    assert!(
        before.iter().all(|count| *count > 0) && tree > 0,
        "every layer drew: {before:?}"
    );
    native.simulate_keystrokes("tab");
    frame(&mut native);
    let into = focused(&mut native).unwrap_or_default();
    assert!(
        into.starts_with("console:pane/0/"),
        "Tab from the bar's end went to {into:?}"
    );
    let drawn: Vec<u64> = counts()
        .iter()
        .zip(before)
        .map(|(after, before)| after - before)
        .collect();
    let root = drawn[0];
    assert!(root > 0, "the focus move drew no frame");
    assert_eq!(
        drawn, [root; 6],
        "under AT each layer draws with the root: [root, pane view, bar, strip, dialogs, footer]"
    );
    assert_eq!(
        tree_renders(PROGRAM) - tree,
        root,
        "under AT the focus move drew the tree with the root"
    );
}

#[gpui_kit::test]
fn a_pop_out_draws_panes_and_footer_and_no_bar(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    settle(&mut native);
    let (popped_key, popped_handle, _) = pop_out(&app, key, &view, &mut native);
    let nodes = native.update(|_, cx| {
        popped_handle
            .update(cx, |_, window, cx| {
                window.simulate_next_frame(cx);
                draw(window, cx)
            })
            .unwrap()
    });
    let ids = ids(&nodes);
    assert!(
        ids.iter().any(|id| id == "console:pane/0/view"),
        "no pane in the pop-out: {nodes}"
    );
    assert!(
        !ids.iter().any(|id| id == "console:menubar"),
        "a bar in the pop-out: {nodes}"
    );
    assert!(
        window_count(popped_key, "renders.panes") > 0,
        "the pop-out drew no panes"
    );
    assert!(
        window_count(popped_key, "renders.toast") > 0,
        "the pop-out drew no footer"
    );
    assert_eq!(
        window_count(popped_key, "renders.chrome"),
        0,
        "the pop-out drew a bar"
    );
    assert_eq!(
        window_count(popped_key, "renders.dot"),
        0,
        "the pop-out drew a dot"
    );
}

#[gpui_kit::test]
fn search_from_a_pop_out_opens_spotlight_on_the_console(cx: &mut TestAppContext) {
    let (app, key, view, mut native) = console(cx);
    settle(&mut native);
    let (popped_key, popped_handle, _) = pop_out(&app, key, &view, &mut native);
    native.update(|_, cx| {
        popped_handle
            .update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(crate::shell::keys::ToggleSpotlight), cx)
            })
            .unwrap()
    });
    native.run_until_parked();
    native.update(|_, cx| {
        let open = |key| {
            *app.windows
                .read(cx)
                .own(key)
                .unwrap()
                .overlays
                .read(cx)
                .get()
        };
        assert_eq!(
            open(key),
            Some(Overlay::Spotlight),
            "the console shows no Spotlight"
        );
        assert_eq!(
            open(popped_key),
            None,
            "the pop-out holds a Spotlight it cannot draw"
        );
    });
}

#[gpui_kit::test]
fn the_launcher_is_the_console_root_before_the_desk(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let mut seed = Seed::boot();
    seed.center = Default::default();
    seed.roster = Default::default();
    seed.account.key_exists = true;
    let (_, key, view, mut native) = open_console(seed, cx);
    let nodes = native.update(draw);
    let shown = ids(&nodes);
    assert!(
        shown.iter().any(|id| id == "console:connect"),
        "no connect screen: {nodes}"
    );
    assert!(
        !shown.iter().any(|id| id == "console:menubar"),
        "a bar before the desk: {nodes}"
    );
    let launcher = window_count(key, "renders.launcher");
    assert!(launcher > 0, "the launcher never drew");
    assert_eq!(
        window_count(key, "renders.chrome"),
        0,
        "the bar drew before the desk"
    );
    set_screen(&view, Screen::Unlock { awaiting: false }, &mut native);
    frame(&mut native);
    assert_eq!(
        window_count(key, "renders.launcher"),
        launcher + 2,
        "a screen change and its figure drew the launcher other than twice"
    );
    let nodes = native.update(draw);
    assert!(
        ids(&nodes).iter().any(|id| id == "console:unlock"),
        "no unlock screen: {nodes}"
    );
    assert_eq!(
        window_count(key, "renders.chrome"),
        0,
        "the bar drew before the desk"
    );
    set_screen(&view, Screen::Desk, &mut native);
    frame(&mut native);
    let nodes = native.update(draw);
    assert!(
        ids(&nodes).iter().any(|id| id == "console:menubar"),
        "no bar on the desk: {nodes}"
    );
    assert!(
        window_count(key, "renders.chrome") > 0,
        "the desk drew no bar"
    );
}

#[gpui_kit::test]
fn the_figure_follows_the_screen_without_a_render_write(cx: &mut TestAppContext) {
    use crate::shell::figure::Figure;
    let _on = crate::perf::on_for_test();
    let mut seed = Seed::boot();
    seed.center = Default::default();
    seed.roster = Default::default();
    seed.prefs.motion = false;
    let (_, key, view, mut native) = open_console(seed, cx);
    frame(&mut native);
    let spin = native.update(|_, cx| view.read(cx).launcher().read(cx).spin.clone());
    let drawn = |native: &mut VisualTestContext| native.update(|_, cx| spin.read(cx).drawn());
    assert_eq!(
        drawn(&mut native),
        Some(Figure::Roll),
        "Connect draws the roll"
    );
    let launcher = window_count(key, "renders.launcher");

    set_screen(&view, Screen::Unlock { awaiting: false }, &mut native);
    native.run_until_parked();
    assert_eq!(
        (drawn(&mut native), window_count(key, "renders.launcher")),
        (Some(Figure::Roll), launcher + 1),
        "the frame showing the key screen drew other than the last figure, once"
    );
    frame(&mut native);
    assert_eq!(
        (drawn(&mut native), window_count(key, "renders.launcher")),
        (Some(Figure::Ring), launcher + 2),
        "the frame after it drew other than the key screen's figure, once"
    );
    frame(&mut native);
    assert_eq!(
        window_count(key, "renders.launcher"),
        launcher + 2,
        "a third frame followed the screen change"
    );
}

#[gpui_kit::test]
fn a_figure_back_from_the_desk_comes_with_its_screen(cx: &mut TestAppContext) {
    use crate::shell::figure::Figure;
    let mut seed = Seed::boot();
    seed.center = Default::default();
    seed.roster = Default::default();
    seed.prefs.motion = false;
    let (_, _, view, mut native) = open_console(seed, cx);
    let spin = native.update(|_, cx| view.read(cx).launcher().read(cx).spin.clone());
    let drawn = |native: &mut VisualTestContext| native.update(|_, cx| spin.read(cx).drawn());
    for screen in [Screen::Unlock { awaiting: false }, Screen::Desk] {
        set_screen(&view, screen, &mut native);
        frame(&mut native);
        frame(&mut native);
    }
    assert_eq!(
        drawn(&mut native),
        Some(Figure::Ring),
        "the key screen drew"
    );
    set_screen(&view, Screen::Connect, &mut native);
    native.run_until_parked();
    assert_eq!(
        drawn(&mut native),
        Some(Figure::Roll),
        "the connect screen came back with the key screen's figure"
    );
}

#[gpui_kit::test]
fn a_toast_paints_over_an_open_menu(cx: &mut TestAppContext) {
    let (app, _, view, mut native) = console(cx);
    super::tests::show(&view, Some(Overlay::Menu(Popover::Node)), &mut native);
    toast(&app, "Saved", &mut native);
    settle(&mut native);
    let nodes = native.update(draw);
    let ids = ids(&nodes);
    let at = |id: &str| {
        ids.iter()
            .position(|it| it == id)
            .unwrap_or_else(|| panic!("no {id}: {nodes}"))
    };
    assert!(
        at("console:toast") > at("console:node-status"),
        "the toast is under the menu: {ids:?}"
    );
}

#[gpui_kit::test]
fn the_centre_hears_the_front_pane_without_a_frame(cx: &mut TestAppContext) {
    const PROGRAM: &str = "root-front-view";
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_for_test(PROGRAM, 400);
    native.update(|window, _| window.activate_window());
    settle(&mut native);
    let front = |native: &mut VisualTestContext| {
        native.update(|_, cx| app.notifications.read(cx).center().lock().front)
    };
    assert_ne!(front(&mut native), Some((key, PROGRAM)));
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    assert_eq!(front(&mut native), Some((key, PROGRAM)));
}

#[gpui_kit::test]
fn a_view_is_visible_only_while_its_window_is_in_front(cx: &mut TestAppContext) {
    const PROGRAM: &str = "root-visible-view";
    let (_, _, view, mut native) = console(cx);
    crate::runtime::seat_for_test(PROGRAM, 400);
    native.update(|window, _| window.activate_window());
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    assert!(
        crate::runtime::visible_for_test(PROGRAM),
        "the window in front shows its pane"
    );
    native.deactivate_window();
    settle(&mut native);
    assert!(
        !crate::runtime::visible_for_test(PROGRAM),
        "a window the person left still read as looked at"
    );
    native.update(|window, _| window.activate_window());
    settle(&mut native);
    assert!(crate::runtime::visible_for_test(PROGRAM), "back in front");
}

#[gpui_kit::test]
fn a_pane_move_between_windows_keeps_its_seat_and_tree(cx: &mut TestAppContext) {
    const PROGRAM: &str = "root-move-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("kept"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    let instance = native.update(|_, cx| view.read(cx).layout(cx).panes[0].instance);
    let seat = native.update(|_, cx| app.seats.read(cx).seat(instance).expect("seated"));
    let console_handle = native.update(|_, cx| app.windows.read(cx).handles()[&key]);
    assert_eq!(
        seat.read_with(&native, |seat, _| seat.window()),
        Some(console_handle)
    );
    let drawn = tree_renders(PROGRAM);
    assert!(drawn > 0, "the tree drew in the console");

    let (popped_key, popped_handle, popped) = pop_out(&app, key, &view, &mut native);
    native.update(|_, cx| {
        popped_handle
            .update(cx, |_, window, cx| {
                window.simulate_next_frame(cx);
                draw(window, cx);
            })
            .unwrap();
    });
    native.run_until_parked();
    native.update(|_, cx| {
        assert_eq!(
            popped.read(cx).layout(cx).panes[0].instance,
            instance,
            "the pane changed instance on its way out"
        );
        assert_eq!(
            app.seats.read(cx).seat(instance).map(|it| it.entity_id()),
            Some(seat.entity_id()),
            "the pop-out got a new seat"
        );
        assert_eq!(seat.read(cx).window(), Some(popped_handle));
    });
    assert!(
        window_count(popped_key, "renders.panes") > 0,
        "the pop-out drew no panes"
    );

    app.windows
        .update(&mut native, |windows, cx| windows.pop_in(popped_key, cx));
    native.run_until_parked();
    native.update(|_, cx| {
        assert!(
            view.read(cx)
                .layout(cx)
                .panes
                .iter()
                .any(|pane| pane.instance == instance),
            "the pane never came back"
        );
        assert_eq!(
            app.seats.read(cx).seat(instance).map(|it| it.entity_id()),
            Some(seat.entity_id()),
            "the return got a new seat"
        );
        assert_eq!(seat.read(cx).window(), Some(console_handle));
        assert!(
            !app.windows.read(cx).handles().contains_key(&popped_key),
            "the pop-out's window stayed"
        );
    });
    frame(&mut native);
    assert!(
        tree_renders(PROGRAM) > drawn,
        "the tree never drew again after its moves"
    );
}

#[gpui_kit::test]
fn raising_an_active_window_closes_its_switch_timer(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, _, mut native) = console(cx);
    native.update(|window, _| window.activate_window());
    settle(&mut native);
    let samples = |native: &mut VisualTestContext| {
        native.run_until_parked();
        crate::perf::snapshot(false)["windows"][key.0.to_string()]["switch"]["n"]
            .as_u64()
            .unwrap_or(0)
    };
    let before = samples(&mut native);
    app.windows
        .update(&mut native, |windows, cx| windows.raise(key, cx));
    frame(&mut native);
    frame(&mut native);
    assert_eq!(
        samples(&mut native),
        before + 1,
        "the switch timer stayed open"
    );
}

fn cached_frame_quads_without_reader(native: &mut VisualTestContext) -> Vec<gpui_kit::Quad> {
    native.update(|window, _| window.painted_quads())
}

#[gpui_kit::test]
fn a_cached_dialog_dims_the_panes(cx: &mut TestAppContext) {
    const PROGRAM: &str = "root-scrim-view";
    let (app, _, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(PROGRAM, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(PROGRAM), &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    for overlay in [Overlay::Spotlight, Overlay::Approve] {
        super::tests::show(&view, Some(overlay), &mut native);
        for _ in 0..3 {
            frame(&mut native);
            let quads = cached_frame_quads_without_reader(&mut native);
            let scale = native.update(|window, _| window.scale_factor());
            let scrim = quads
                .iter()
                .find(|quad| {
                    quad.background
                        .as_solid()
                        .is_some_and(|color| (color.a - 0.6).abs() < 0.01)
                })
                .unwrap_or_else(|| panic!("{overlay:?}'s scrim paints"));
            assert_eq!(
                (
                    scrim.bounds.origin.y.0,
                    scrim.bounds.size.width.0,
                    scrim.bounds.size.height.0,
                ),
                (BAR * scale, 1280. * scale, (800. - BAR) * scale),
                "{overlay:?}'s scrim fills the window under the bar"
            );
            let pane = quads
                .iter()
                .filter(|quad| {
                    quad.border_widths.top.0 == scale && quad.bounds.size.height.0 > 400. * scale
                })
                .map(|quad| quad.order)
                .max()
                .expect("a pane paints");
            assert!(
                scrim.order > pane,
                "{overlay:?}'s scrim {} under a pane {pane}",
                scrim.order
            );
        }
        super::tests::show(&view, None, &mut native);
        frame(&mut native);
    }
}

#[gpui_kit::test]
fn a_dialog_follows_the_chrome_when_the_layout_switches(cx: &mut TestAppContext) {
    use crate::backend::Layout;
    let (app, _, view, mut native) = console(cx);
    set_motion(&app, false, &mut native);
    let settings = Overlay::Settings(Default::default());
    super::tests::show(&view, Some(settings), &mut native);
    let placed = |native: &mut VisualTestContext| {
        for _ in 0..3 {
            frame(native);
        }
        let scale = native.update(|window, _| window.scale_factor());
        let scrim = cached_frame_quads_without_reader(native)
            .into_iter()
            .find(|quad| {
                quad.background
                    .as_solid()
                    .is_some_and(|color| (color.a - 0.6).abs() < 0.01)
            })
            .expect("the scrim paints")
            .bounds;
        let scrim = [
            scrim.origin.x,
            scrim.origin.y,
            scrim.size.width,
            scrim.size.height,
        ]
        .map(|it| it.0 / scale);
        let nodes = native.update(|window, cx| {
            draw(window, cx);
            serde_json::to_value(crate::ax::snapshot("console", window, true)).unwrap()
        });
        let card = nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "console:settings-window")
            .expect("Settings is open");
        let at = |n: usize| card["bounds"][n].as_f64().unwrap() as f32;
        (scrim, [at(0), at(1)])
    };
    let switch = |layout, native: &mut VisualTestContext| {
        app.prefs
            .update(native, |prefs, cx| prefs.set_layout(layout, cx))
    };
    assert_eq!(
        placed(&mut native),
        ([0., BAR, 1280., 800. - BAR], [260., BAR + 72.]),
        "under the bar"
    );
    switch(Layout::Sidebar, &mut native);
    assert_eq!(
        placed(&mut native),
        ([220., 0., 1060., 800.], [220. + 150., 74.]),
        "right of the sidebar"
    );
    switch(Layout::MenuBar, &mut native);
    assert_eq!(
        placed(&mut native),
        ([0., BAR, 1280., 800. - BAR], [260., BAR + 72.]),
        "under the bar again"
    );
    assert_eq!(super::tests::open_now(&view, &mut native), Some(settings));
}

#[gpui_kit::test]
fn a_cached_toast_sits_at_the_foot(cx: &mut TestAppContext) {
    let (app, _, _, mut native) = console(cx);
    set_motion(&app, false, &mut native);
    toast(&app, "Saved", &mut native);
    for _ in 0..3 {
        frame(&mut native);
        let scale = native.update(|window, _| window.scale_factor());
        let toast = cached_frame_quads_without_reader(&mut native)
            .into_iter()
            .find(|quad| {
                quad.bounds.size.width.0 == 560. * scale && quad.border_widths.top.0 == 1.5 * scale
            })
            .expect("the toast's bar paints");
        let foot = (toast.bounds.origin.y + toast.bounds.size.height).0;
        assert!(
            (foot - (800. - 32.) * scale).abs() < 1.,
            "the toast's foot at {foot}, not 32px over the window's"
        );
    }
}

#[gpui_kit::test]
fn a_views_deferred_draw_stays_under_a_dialog(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    use view_wire as wire;
    const MAGENTA: u32 = 0xff00ff;
    let lookalike = || {
        let mut style = gpui_kit::div()
            .absolute()
            .inset_0()
            .bg(gpui_kit::rgb(MAGENTA));
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("lookalike".into())),
            style: crate::render::test_style(style.style().clone()),
            interactivity: Some(Box::new(wire::Interactivity {
                on_click: Some(1),
                occlude: true,
                ..Default::default()
            })),
            children: Vec::new(),
        })
    };
    let deferred = |content| wire::Node::Deferred {
        priority: 16,
        content: Box::new(content),
    };
    let views = [
        ("root-deferred-view", deferred(lookalike())),
        ("root-nested-view", deferred(deferred(lookalike()))),
    ];
    for (program, root) in views {
        let (app, _, view, mut native) = console(cx);
        let root = wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("slot".into())),
            style: crate::render::test_style(
                gpui_kit::div().size_full().relative().style().clone(),
            ),
            interactivity: Default::default(),
            children: vec![root],
        });
        crate::runtime::seat_drawing_for_test(program, 400, root);
        set_motion(&app, false, &mut native);
        pane(&view, PaneMessage::Select(program), &mut native);
        for _ in 0..4 {
            frame(&mut native);
        }
        super::tests::show(&view, Some(Overlay::Approve), &mut native);
        for _ in 0..3 {
            frame(&mut native);
        }
        let quads = cached_frame_quads_without_reader(&mut native);
        let scale = native.update(|window, _| window.scale_factor());
        let drawn = quads
            .iter()
            .find(|quad| quad.background.as_solid() == Some(gpui_kit::rgb(MAGENTA).into()))
            .unwrap_or_else(|| panic!("{program}: the view's deferred draw paints"));
        let scrim = quads
            .iter()
            .find(|quad| {
                quad.background
                    .as_solid()
                    .is_some_and(|color| (color.a - 0.6).abs() < 0.01)
            })
            .expect("the dialog's scrim paints");
        assert!(
            drawn.order < scrim.order,
            "{program}: the view's draw {} over the scrim {}",
            drawn.order,
            scrim.order
        );
        let at = gpui_kit::point(
            gpui_kit::px(drawn.bounds.origin.x.0 / scale + 8.),
            gpui_kit::px((drawn.bounds.origin.y.0 + drawn.bounds.size.height.0) / scale - 8.),
        );
        native.simulate_click(at, gpui_kit::Modifiers::none());
        frame(&mut native);
        let open = native.update(|_, cx| *view.read(cx).overlays().read(cx).get());
        assert_eq!(open, None, "{program}: the view took the scrim's click");
    }
}

#[gpui_kit::test]
fn a_dialogs_scrim_over_a_views_tooltip_source_closes_the_tooltip(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    use view_wire as wire;
    const MAGENTA: u32 = 0xff00ff;
    const CYAN: u32 = 0x00ffff;
    for (program, hoverable) in [("root-tip-hoverable", true), ("root-tip-plain", false)] {
        let (app, _, view, mut native) = console(cx);
        let mut tip = gpui_kit::div()
            .size(gpui_kit::px(3000.))
            .bg(gpui_kit::rgb(MAGENTA));
        let tip = wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("tip".into())),
            style: crate::render::test_style(tip.style().clone()),
            interactivity: Some(Box::new(wire::Interactivity {
                on_click: Some(1),
                occlude: true,
                ..Default::default()
            })),
            children: Vec::new(),
        });
        let mut source = gpui_kit::div().size_full().bg(gpui_kit::rgb(CYAN));
        let root = wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("source".into())),
            style: crate::render::test_style(source.style().clone()),
            interactivity: Some(Box::new(wire::Interactivity {
                tooltip: Some(wire::Tooltip {
                    request: 7,
                    hoverable,
                    delay_ms: 10,
                }),
                ..Default::default()
            })),
            children: Vec::new(),
        });
        crate::runtime::seat_frame_for_test(program, 400, tooltip_frame(root, 7, tip));
        set_motion(&app, false, &mut native);
        pane(&view, PaneMessage::Select(program), &mut native);
        for _ in 0..4 {
            frame(&mut native);
        }
        let scale = native.update(|window, _| window.scale_factor());
        let painted = |native: &mut VisualTestContext, color: u32| {
            cached_frame_quads_without_reader(native)
                .into_iter()
                .find(|quad| quad.background.as_solid() == Some(gpui_kit::rgb(color).into()))
        };
        let pane = painted(&mut native, CYAN)
            .unwrap_or_else(|| panic!("{program}: the view paints"))
            .bounds;
        let middle = gpui_kit::point(
            gpui_kit::px((pane.origin.x.0 + pane.size.width.0 / 2.) / scale),
            gpui_kit::px((pane.origin.y.0 + pane.size.height.0 / 2.) / scale),
        );
        native.simulate_mouse_move(middle, None, Default::default());
        native
            .executor()
            .advance_clock(std::time::Duration::from_millis(11));
        native.run_until_parked();
        for _ in 0..3 {
            frame(&mut native);
        }
        assert!(
            painted(&mut native, MAGENTA).is_some(),
            "{program}: the tooltip opened"
        );
        super::tests::show(&view, Some(Overlay::Approve), &mut native);
        for _ in 0..3 {
            frame(&mut native);
        }
        let quads = cached_frame_quads_without_reader(&mut native);
        let scrim = quads
            .iter()
            .find(|quad| {
                quad.background
                    .as_solid()
                    .is_some_and(|color| (color.a - 0.6).abs() < 0.01)
            })
            .expect("the dialog's scrim paints");
        let tip = quads
            .iter()
            .find(|quad| quad.background.as_solid() == Some(gpui_kit::rgb(MAGENTA).into()));
        assert!(
            tip.is_none(),
            "{program}: the dialog's scrim {} covers the source, yet its tooltip paints at {:?}",
            scrim.order,
            tip.map(|tip| tip.order)
        );
        let at = gpui_kit::point(
            gpui_kit::px(pane.origin.x.0 / scale + 8.),
            gpui_kit::px((pane.origin.y.0 + pane.size.height.0) / scale - 8.),
        );
        native.simulate_click(at, gpui_kit::Modifiers::none());
        frame(&mut native);
        let open = native.update(|_, cx| *view.read(cx).overlays().read(cx).get());
        assert_eq!(open, None, "{program}: the tooltip took the scrim's click");
    }
}

#[gpui_kit::test]
fn a_views_deferred_draw_takes_no_click_outside_its_pane(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    use std::{cell::RefCell, rc::Rc};
    use view_wire as wire;
    const HOSTILE: &str = "root-hostile-view";
    const NEIGHBOUR: &str = "root-neighbour-view";
    const CYAN: u32 = 0x00ffff;
    let mut cover = gpui_kit::div()
        .w(gpui_kit::px(1280.))
        .h(gpui_kit::px(800.))
        .bg(gpui_kit::rgb(0xff00ff));
    let cover = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("cover".into())),
        style: crate::render::test_style(cover.style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            on_click: Some(1),
            occlude: true,
            ..Default::default()
        })),
        children: Vec::new(),
    });
    let hostile = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("slot".into())),
        style: crate::render::test_style(gpui_kit::div().size_full().relative().style().clone()),
        interactivity: Default::default(),
        children: vec![wire::Node::Deferred {
            priority: 16,
            content: Box::new(wire::Node::Anchored {
                anchor: wire::Anchor::TopLeft,
                fit: wire::AnchoredFitMode::SnapToWindow,
                position: Some([0., 0.]),
                position_mode: wire::AnchoredPositionMode::Window,
                offset: None,
                children: vec![cover],
            }),
        }],
    });
    let mut neighbour = gpui_kit::div().size_full().bg(gpui_kit::rgb(CYAN));
    let neighbour = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("neighbour".into())),
        style: crate::render::test_style(neighbour.style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            on_click: Some(2),
            ..Default::default()
        })),
        children: Vec::new(),
    });
    crate::runtime::seat_drawing_for_test(HOSTILE, 400, hostile);
    crate::runtime::seat_drawing_for_test(NEIGHBOUR, 400, neighbour);
    let mut seed = Seed::boot();
    seed.roster = Default::default();
    seed.center = Default::default();
    seed.screen = Screen::Desk;
    seed.account.signer_key = "ab".into();
    let (app, _, view, mut native) = open_console(seed, cx);
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(HOSTILE), &mut native);
    pane(&view, PaneMessage::Split(NEIGHBOUR), &mut native);
    for (program, x) in [(HOSTILE, 12.), (NEIGHBOUR, 640.)] {
        let index = native.update(|_, cx| {
            let layout = view.read(cx).layout(cx);
            layout.panes.iter().position(|pane| pane.program == program)
        });
        let frame = crate::ui::layout::Frame {
            x,
            y: 12.,
            w: 600.,
            h: 500.,
        };
        pane(
            &view,
            PaneMessage::Frame(index.expect("the pane opened"), frame),
            &mut native,
        );
    }
    for _ in 0..4 {
        frame(&mut native);
    }
    let clicks = |program: &str, native: &mut VisualTestContext| {
        let clicks = Rc::new(RefCell::new(Vec::new()));
        let seen = clicks.clone();
        native.update(|_, cx| {
            let layout = view.read(cx).layout(cx);
            let pane = layout
                .panes
                .iter()
                .find(|pane| pane.program == program)
                .expect("the pane opened");
            let tree = app.seats.read(cx).seat(pane.instance).expect("seated");
            let tree = tree.read(cx).tree().expect("the view drew");
            cx.subscribe(&tree, move |_, event: &wire::Event, _| {
                if let wire::Event::Click { handler, .. } = event {
                    seen.borrow_mut().push(*handler);
                }
            })
            .detach();
        });
        clicks
    };
    let hostile = clicks(HOSTILE, &mut native);
    let neighbour = clicks(NEIGHBOUR, &mut native);
    let account = native.update(|window, cx| {
        draw(window, cx);
        let nodes = serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap();
        let button = nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "shell:rail-account")
            .cloned()
            .unwrap_or_else(|| panic!("the bar's account button: {nodes}"));
        let at = |n: usize| button["bounds"][n].as_f64().unwrap() as f32;
        gpui_kit::point(
            gpui_kit::px((at(0) + at(2)) / 2.),
            gpui_kit::px((at(1) + at(3)) / 2.),
        )
    });
    native.simulate_click(account, gpui_kit::Modifiers::none());
    frame(&mut native);
    let open = native.update(|_, cx| *view.read(cx).overlays().read(cx).get());
    super::tests::show(&view, None, &mut native);
    frame(&mut native);
    let scale = native.update(|window, _| window.scale_factor());
    let beside = cached_frame_quads_without_reader(&mut native)
        .into_iter()
        .find(|quad| quad.background.as_solid() == Some(gpui_kit::rgb(CYAN).into()))
        .expect("the neighbour paints")
        .bounds;
    let middle = gpui_kit::point(
        gpui_kit::px((beside.origin.x.0 + beside.size.width.0 / 2.) / scale),
        gpui_kit::px((beside.origin.y.0 + beside.size.height.0 / 2.) / scale),
    );
    native.simulate_click(middle, gpui_kit::Modifiers::none());
    frame(&mut native);
    assert_eq!(
        (open, neighbour.take(), hostile.take()),
        (Some(Overlay::Menu(Popover::Account)), vec![2], vec![]),
        "a click outside the view's pane reached its layer"
    );
}

type Clicks = std::rc::Rc<std::cell::RefCell<Vec<u32>>>;

fn tooltip_frame(
    root: view_wire::Node,
    request: u32,
    content: view_wire::Node,
) -> view_wire::Frame {
    view_wire::Frame {
        root: Some(root),
        tooltip_responses: vec![view_wire::TooltipResponse {
            request,
            character_index: None,
            content: Some(Box::new(content)),
        }],
        ..Default::default()
    }
}

fn cascade(
    back: &'static str,
    front: &'static str,
    tree: view_wire::Frame,
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<super::WindowRoot>,
    VisualTestContext,
    Clicks,
    Clicks,
) {
    use gpui_kit::Styled as _;
    use view_wire as wire;
    let mut body = gpui_kit::div().size_full().bg(gpui_kit::rgb(0x00ffff));
    let body = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("front".into())),
        style: crate::render::test_style(body.style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            on_click: Some(2),
            ..Default::default()
        })),
        children: Vec::new(),
    });
    crate::runtime::seat_frame_for_test(back, 400, tree);
    crate::runtime::seat_drawing_for_test(front, 400, body);
    let mut seed = Seed::boot();
    seed.roster = Default::default();
    seed.center = Default::default();
    seed.screen = Screen::Desk;
    let (app, _, view, mut native) = open_console(seed, cx);
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(back), &mut native);
    pane(&view, PaneMessage::Split(front), &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    let (frames, top) = native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        let frame = |program: &str| {
            let pane = layout.panes.iter().find(|pane| pane.program == program);
            pane.and_then(|pane| pane.frame).expect("on the desk")
        };
        let top = *layout.stacking().last().expect("two windows");
        ((frame(back), frame(front)), layout.panes[top].program)
    });
    assert_eq!(top, front, "the split is the front window");
    assert_eq!(
        (frames.1.x - frames.0.x, frames.1.y - frames.0.y),
        (28., 28.),
        "the split cascades over the window under it"
    );
    let clicks = |program: &str, native: &mut VisualTestContext| {
        let clicks = Clicks::default();
        let seen = clicks.clone();
        native.update(|_, cx| {
            let layout = view.read(cx).layout(cx);
            let pane = layout
                .panes
                .iter()
                .find(|pane| pane.program == program)
                .expect("the pane opened");
            let tree = app.seats.read(cx).seat(pane.instance).expect("seated");
            let tree = tree.read(cx).tree().expect("the view drew");
            cx.subscribe(&tree, move |_, event: &wire::Event, _| {
                if let wire::Event::Click { handler, .. } = event {
                    seen.borrow_mut().push(*handler);
                }
            })
            .detach();
        });
        clicks
    };
    let (back, front) = (clicks(back, &mut native), clicks(front, &mut native));
    (view, native, back, front)
}

fn front_body(
    native: &mut VisualTestContext,
) -> (gpui_kit::Quad, gpui_kit::Point<gpui_kit::Pixels>) {
    let scale = native.update(|window, _| window.scale_factor());
    let body = cached_frame_quads_without_reader(native)
        .into_iter()
        .find(|quad| quad.background.as_solid() == Some(gpui_kit::rgb(0x00ffff).into()))
        .expect("the front window paints");
    let middle = gpui_kit::point(
        gpui_kit::px((body.bounds.origin.x.0 + body.bounds.size.width.0 / 2.) / scale),
        gpui_kit::px((body.bounds.origin.y.0 + body.bounds.size.height.0 / 2.) / scale),
    );
    (body, middle)
}

#[gpui_kit::test]
fn a_back_windows_deferred_draw_takes_no_click_on_the_front_window(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    use view_wire as wire;
    let mut cover = gpui_kit::div()
        .w(gpui_kit::px(1280.))
        .h(gpui_kit::px(800.))
        .bg(gpui_kit::rgb(0xff00ff));
    let cover = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("cover".into())),
        style: crate::render::test_style(cover.style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            on_click: Some(1),
            occlude: true,
            ..Default::default()
        })),
        children: Vec::new(),
    });
    let hostile = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("slot".into())),
        style: crate::render::test_style(gpui_kit::div().size_full().relative().style().clone()),
        interactivity: Default::default(),
        children: vec![wire::Node::Deferred {
            priority: 16,
            content: Box::new(wire::Node::Anchored {
                anchor: wire::Anchor::TopLeft,
                fit: wire::AnchoredFitMode::SnapToWindow,
                position: Some([0., 0.]),
                position_mode: wire::AnchoredPositionMode::Window,
                offset: None,
                children: vec![cover],
            }),
        }],
    });
    let (_, mut native, back, front) = cascade(
        "root-cascade-hostile-view",
        "root-cascade-front-view",
        view_wire::Frame {
            root: Some(hostile),
            ..Default::default()
        },
        cx,
    );
    let (_, middle) = front_body(&mut native);
    native.simulate_click(middle, gpui_kit::Modifiers::none());
    frame(&mut native);
    assert_eq!(
        (front.take(), back.take()),
        (vec![2], vec![]),
        "the front window's click reached the back window's layer"
    );
}

#[gpui_kit::test]
fn a_back_windows_tooltip_stays_under_the_front_window(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    use view_wire as wire;
    const MAGENTA: u32 = 0xff00ff;
    const YELLOW: u32 = 0xffff00;
    let mut tip = gpui_kit::div()
        .size(gpui_kit::px(3000.))
        .bg(gpui_kit::rgb(MAGENTA));
    let tip = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("tip".into())),
        style: crate::render::test_style(tip.style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            on_click: Some(1),
            occlude: true,
            ..Default::default()
        })),
        children: Vec::new(),
    });
    let mut source = gpui_kit::div().size_full().bg(gpui_kit::rgb(YELLOW));
    let source = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("source".into())),
        style: crate::render::test_style(source.style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            tooltip: Some(wire::Tooltip {
                request: 7,
                hoverable: true,
                delay_ms: 10,
            }),
            ..Default::default()
        })),
        children: Vec::new(),
    });
    let (_, mut native, back, front) = cascade(
        "root-cascade-tip-view",
        "root-cascade-tip-front-view",
        tooltip_frame(source, 7, tip),
        cx,
    );
    let scale = native.update(|window, _| window.scale_factor());
    let painted = |native: &mut VisualTestContext, color: u32| {
        cached_frame_quads_without_reader(native)
            .into_iter()
            .find(|quad| quad.background.as_solid() == Some(gpui_kit::rgb(color).into()))
    };
    let source = painted(&mut native, YELLOW)
        .expect("the back view paints")
        .bounds;
    let bare = gpui_kit::point(
        gpui_kit::px(source.origin.x.0 / scale + 8.),
        gpui_kit::px((source.origin.y.0 + source.size.height.0 / 2.) / scale),
    );
    native.simulate_mouse_move(bare, None, Default::default());
    native
        .executor()
        .advance_clock(std::time::Duration::from_millis(11));
    native.run_until_parked();
    for _ in 0..3 {
        frame(&mut native);
    }
    let tip = painted(&mut native, MAGENTA).expect("the tooltip opened");
    let (body, middle) = front_body(&mut native);
    assert!(
        tip.order < body.order,
        "the back window's tooltip {} over the front window {}",
        tip.order,
        body.order
    );
    native.simulate_click(middle, gpui_kit::Modifiers::none());
    frame(&mut native);
    assert_eq!(
        (front.take(), back.take()),
        (vec![2], vec![]),
        "the front window's click reached the back window's tooltip"
    );
}

#[gpui_kit::test]
fn settings_opens_over_the_launcher(cx: &mut TestAppContext) {
    use super::tests::open_now;
    let (app, _, view, mut native) = open_console(Seed::boot(), cx);
    set_motion(&app, false, &mut native);
    native.simulate_keystrokes("secondary-,");
    assert_eq!(
        open_now(&view, &mut native),
        Some(Overlay::Settings(Default::default()))
    );
    for _ in 0..3 {
        frame(&mut native);
    }
    let scale = native.update(|window, _| window.scale_factor());
    let scrim = cached_frame_quads_without_reader(&mut native)
        .into_iter()
        .find(|quad| {
            quad.background
                .as_solid()
                .is_some_and(|color| (color.a - 0.6).abs() < 0.01)
        })
        .expect("the scrim paints over the launcher")
        .bounds;
    assert_eq!(
        [
            scrim.origin.x,
            scrim.origin.y,
            scrim.size.width,
            scrim.size.height,
        ]
        .map(|it| it.0 / scale),
        match cfg!(target_os = "macos") {
            true => [0., 36., 1280., 764.],
            false => [0., 0., 1280., 800.],
        }
    );
    let nodes = native.update(|window, cx| {
        draw(window, cx);
        serde_json::to_value(crate::ax::snapshot("console", window, true)).unwrap()
    });
    assert!(
        nodes
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == "console:settings-window"),
        "Settings is drawn"
    );
    native.simulate_keystrokes("escape");
    assert_eq!(open_now(&view, &mut native), None);
}

#[gpui_kit::test]
fn an_idle_window_takes_no_frames(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, _, mut native) = console(cx);
    native.update(|_, cx| cx.set_reduce_motion(false));
    set_motion(&app, false, &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    let drawn = window_count(key, "renders");
    assert_eq!(
        ducktape_host::frames::deliver(&mut native, std::time::Duration::from_secs(1)),
        0,
        "an idle window took frames"
    );
    assert_eq!(window_count(key, "renders"), drawn, "an idle window drew");
}
