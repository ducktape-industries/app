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

/// The node's breath is a view of its own: with motion on, each of its
/// pulses draws the dot and the window's root, and the bar (cached, with
/// its well empty) not at all.
#[gpui_kit::test]
fn a_pulse_re_renders_the_dot_and_not_the_chrome(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, _, mut native) = console(cx);
    native.update(|_, cx| cx.set_reduce_motion(false));
    native.update(|window, _| window.activate_window());
    set_motion(&app, true, &mut native);
    polled(&app, status(7), &mut native);
    // frames, not `draw`: its `activate_a11y` refreshes the window, a miss
    // for every cached layer
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
    // the pulse paces itself at 30 fps on a timer: each step lands one
    for _ in 0..10 {
        native
            .executor()
            .advance_clock(std::time::Duration::from_millis(40));
        native.run_until_parked();
    }
    // the test clock jitters forward a little on each poll, so a step can
    // land two pulses: at least ten, and the root drew once per pulse
    let pulsed = count(&mut native, "renders.dot") - dot;
    assert!(pulsed >= 10, "the dot pulsed {pulsed} times in ten steps");
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

/// Every render counter window `key` has, by name.
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

/// An idle app on a chain that stands still draws nothing: the node's
/// status poll runs on its own clock and lands the same height (and, once,
/// no answer), and no layer of the window draws, nor the root that lays
/// them out, nor a pane's tree. No clock of the app's draws it otherwise:
/// the wall clock's beat went with the reducer, and its `shell.dispatch`
/// timer with it.
#[gpui_kit::test]
fn a_still_chain_draws_no_frame(cx: &mut TestAppContext) {
    use crate::shell::entities::{STATUS_EVERY, SessionState};
    const MODULE: &str = "root-still-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    // the node answers height 7 to every ask but the fourth
    let (source, asked) = source(|n| match n {
        3 => Err("no answer".to_owned()),
        _ => Ok(status(7)),
    });
    app.session.update(&mut native, |session, cx| {
        let on_node = SessionState {
            connected: true,
            network: "testkit".into(),
            chain: "testkit".into(),
            ..SessionState::booted()
        };
        session.seed_connected(on_node, source, cx)
    });
    let poll = |native: &mut VisualTestContext| {
        native.executor().advance_clock(STATUS_EVERY);
        native.run_until_parked();
    };
    // the first answer moves the height from none to 7, and draws it
    poll(&mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    let still = render_counts(key);
    let tree = tree_renders(MODULE);
    assert!(
        still
            .iter()
            .any(|(name, count)| name == "renders.chrome" && *count > 0),
        "the bar never drew: {still:?}"
    );
    let before = asked.get();
    for _ in 0..6 {
        poll(&mut native);
    }
    assert_eq!(asked.get(), before + 6, "the poll did not run");
    assert_eq!(
        render_counts(key),
        still,
        "a poll that moved nothing drew the window"
    );
    assert_eq!(
        tree_renders(MODULE),
        tree,
        "a still chain drew a pane's tree"
    );
    assert!(
        crate::perf::snapshot(false)["shell"]
            .get("dispatch")
            .is_none(),
        "something still times a reducer dispatch"
    );
}

/// A seat that wakes (its view lands again) draws its own tree and leaves
/// the bar cached.
#[gpui_kit::test]
fn a_pane_wake_leaves_the_chrome_alone(cx: &mut TestAppContext) {
    const MODULE: &str = "root-wake-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("first"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    for _ in 0..8 {
        frame(&mut native);
    }
    let (chrome, tree) = (window_count(key, "renders.chrome"), tree_renders(MODULE));
    assert!(chrome > 0 && tree > 0, "the bar and the tree drew");
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("again"));
    for _ in 0..4 {
        frame(&mut native);
    }
    assert_eq!(
        tree_renders(MODULE),
        tree + 1,
        "the woken tree drew other than once"
    );
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome,
        "a pane's wake drew the bar"
    );
}

/// A toast showing draws the footer's layer, and neither the bar nor a
/// pane's tree.
#[gpui_kit::test]
fn a_toast_re_renders_its_layer_only(cx: &mut TestAppContext) {
    const MODULE: &str = "root-toast-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    for _ in 0..8 {
        frame(&mut native);
    }
    let (chrome, shown, tree) = (
        window_count(key, "renders.chrome"),
        window_count(key, "renders.toast"),
        tree_renders(MODULE),
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
    assert_eq!(tree_renders(MODULE), tree, "a toast drew a pane's tree");
    // the door's tree after the counts: activating it refreshes the window
    let nodes = native.update(draw);
    assert!(
        ids(&nodes).iter().any(|id| id == "console:toast"),
        "no toast up: {nodes}"
    );
}

/// The node in `window`'s last tree that the element with its own id
/// `name` built.
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

/// Each named element's node in `window`'s last tree, with its parent's
/// element path (`None` under the window's root node).
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

/// What the door serves of the console: its node ids, in order.
fn door_ids(native: &mut VisualTestContext) -> Vec<String> {
    native.update(|window, _| {
        crate::ax::snapshot("console", window, false)
            .into_iter()
            .map(|node| node.id)
            .collect()
    })
}

/// A cached layer stays cached under a reader, and its nodes stay in the
/// tree (the fork carries a reused view's nodes, focus ids and listeners):
/// with a11y on, ten pulses of the dot, each followed by a door read (a
/// draw, as `ax::actions::current` draws), draw neither the bar, nor the
/// strip, nor the pane's tree, and the door serves the same nodes after
/// each: the bar, its tabs, the strip's buttons, the view and its tree's
/// text, under the parents the first frame gave them. The pane view draws
/// with every frame and its tree with none: under the door `misses` now
/// fall short of `draws`. The door's Focus on a bar tab drawn from reuse
/// still lands, and a menu hanging off a cached bar keeps its nodes too.
#[gpui_kit::test]
fn a_cached_layer_keeps_its_nodes_under_a11y(cx: &mut TestAppContext) {
    use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
    const MODULE: &str = "root-a11y-view";
    const TAB: &str = "rail/root-a11y-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    app.rail.update(&mut native, |rail, cx| {
        rail.read_off(crate::runtime::Roster::listing(&[MODULE]), cx)
    });
    native.update(|_, cx| cx.set_reduce_motion(false));
    native.update(|window, _| window.activate_window());
    set_motion(&app, true, &mut native);
    polled(&app, status(7), &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    native.update(|window, _| window.activate_a11y());
    for _ in 0..4 {
        frame(&mut native);
    }
    // the strip and the view name no node of their own: their controls
    // and the guest's text are theirs
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
        crate::perf::snapshot(false)["views"][MODULE]["draws"]
            .as_u64()
            .expect("the pane counts its draws")
    };
    let (chrome, strip, tree, drawn) = (
        window_count(key, "renders.chrome"),
        window_count(key, "renders.strip.0"),
        tree_renders(MODULE),
        draws(),
    );
    assert!(chrome > 0 && strip > 0 && tree > 0, "every layer drew");
    let pulse_and_read = |native: &mut VisualTestContext| {
        native
            .executor()
            .advance_clock(std::time::Duration::from_millis(40));
        native.run_until_parked();
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
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome,
        "a pulse or a read drew the bar"
    );
    assert_eq!(
        window_count(key, "renders.strip.0"),
        strip,
        "a pulse or a read drew the strip"
    );
    assert_eq!(
        tree_renders(MODULE),
        tree,
        "a pulse or a read drew the tree"
    );
    assert!(
        draws() >= drawn + 20,
        "the pane view drew {} times in ten pulses and ten reads",
        draws() - drawn
    );
    // the Focus a door `focus` sends, on the tab the last reuse carried
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
    // a menu hangs off the bar (deferred, inside its cache): reuse carries
    // its nodes as the bar's
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
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome,
        "a pulse drew the bar under its menu"
    );
}

/// Typing in Spotlight's field draws the overlay layer (the field is its
/// child: each of the field's own notifies draws it too) and no other
/// cached layer: neither the bar, nor the footer, nor a pane's tree. (The
/// root and the pane layer under it are not cached: they draw with it.)
#[gpui_kit::test]
fn typing_in_spotlight_renders_the_overlay_layer_only(cx: &mut TestAppContext) {
    const MODULE: &str = "root-keystroke-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    super::tests::show(&view, Some(Overlay::Spotlight), &mut native);
    // the first key: the window's input turns to the keyboard, which gpui
    // answers with a full redraw
    native.simulate_input("a");
    for _ in 0..8 {
        frame(&mut native);
    }
    let counts = || {
        (
            window_count(key, "renders.chrome"),
            window_count(key, "renders.toast"),
            tree_renders(MODULE),
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

/// A focus move draws the two views it moves between, not the window (the
/// pinned fork, gpui-pre#9: before it, every focus move refreshed the
/// window and every cached layer missed): Tab from the bar's last control
/// into the pane draws the bar once (its ring goes) and the pane's strip
/// once (its first button's comes), with the root and the pane view under
/// it (neither cached), and neither the pane's tree, nor the dialogs'
/// layer, nor the footer.
#[gpui_kit::test]
fn a_focus_move_re_renders_two_layers_not_the_window(cx: &mut TestAppContext) {
    use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
    const MODULE: &str = "root-focus-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
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
    // the keys on the bar's last control, the door's way; then a Tab back
    // and forth in the bar, as the first key turns the window's input to
    // the keyboard, which gpui answers with a full redraw
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
    let (before, tree) = (counts(), tree_renders(MODULE));
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
        drawn,
        [root, root, 1, 1, 0, 0],
        "[root, pane view, bar, strip, dialogs, footer] drawn"
    );
    assert_eq!(tree_renders(MODULE), tree, "the focus move drew the tree");
}

/// A pane popped out to a window of its own: that window's root draws the
/// panes and the footer, and no bar.
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

/// The View menu's Search goes to whichever window is in front: from a
/// pop-out it opens Spotlight over the console, where it draws, and leaves
/// the pop-out's panes uncovered.
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

/// Before the desk the console's root draws the launcher's screen alone:
/// each screen change draws it once, and once more when the screen's
/// figure follows a frame late
/// (`the_figure_follows_the_screen_without_a_render_write`); no bar until
/// the desk.
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

/// The launcher's figure is never written from a draw: written during the
/// launcher's draw, the figure (a cached view of its own) would keep last
/// frame's drawing until something else drew it again (P3). A screen
/// change draws the launcher with the last screen's figure (its glyphs
/// enter the atlas after the screen's text, which the unlock screen's
/// pixels depend on), the next frame draws the new figure, and no frame
/// follows that.
#[gpui_kit::test]
fn the_figure_follows_the_screen_without_a_render_write(cx: &mut TestAppContext) {
    use crate::shell::figure::Figure;
    let _on = crate::perf::on_for_test();
    let mut seed = Seed::boot();
    seed.center = Default::default();
    seed.roster = Default::default();
    // a still figure: none of its own frames
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

/// Back from the desk the figure is drawn afresh (the desk kept no frame
/// of it), so it comes with its screen: the frame that shows the connect
/// screen draws the roll, not the key screen's ring it last drew.
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

/// A toast paints over an open menu, as it did before it was a layer: its
/// node comes after the menu's in the tree.
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

/// The notification centre hears which view is in front from the root's
/// observers, not from a draw: a pane brought to the front with no frame
/// drawn moves the centre's front.
#[gpui_kit::test]
fn the_centre_hears_the_front_pane_without_a_frame(cx: &mut TestAppContext) {
    const MODULE: &str = "root-front-view";
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_for_test(MODULE, 400);
    native.update(|window, _| window.activate_window());
    settle(&mut native);
    let front = |native: &mut VisualTestContext| {
        native.update(|_, cx| app.notifications.read(cx).center().lock().front)
    };
    assert_ne!(front(&mut native), Some((key, MODULE)));
    // one move of the desk, no frame: the observer alone tells the centre
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    assert_eq!(front(&mut native), Some((key, MODULE)));
}

/// A pane moved between windows (popped out, then back in) keeps its seat
/// and its drawn tree: two roots, one `Seats`, and the seat's place
/// follows the window whose desk holds the pane. Each root draws the pane
/// from the seat it finds by the pane's `instance`.
#[gpui_kit::test]
fn a_pane_move_between_windows_keeps_its_seat_and_tree(cx: &mut TestAppContext) {
    const MODULE: &str = "root-move-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("kept"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
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
    let drawn = tree_renders(MODULE);
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

    // back in: the console's desk holds it again, the seat follows
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
        tree_renders(MODULE) > drawn,
        "the tree never drew again after its moves"
    );
}

/// Raising a window that is already active still times the switch to the
/// frame after the one it asks for: the timer closes and lands a sample.
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
    // the frame the raise asks for, then the one after it
    frame(&mut native);
    frame(&mut native);
    assert_eq!(
        samples(&mut native),
        before + 1,
        "the switch timer stayed open"
    );
}

/// The quads the window painted last, with no reader on: the cached path.
fn quads(native: &mut VisualTestContext) -> Vec<gpui_kit::Quad> {
    native.update(|window, _| window.painted_quads())
}

/// A dialog open over the desk, drawn cached (no reader on): its scrim
/// paints below the bar, the window wide and high, over the panes. The
/// cached view is a layout root of its own, where its height is its
/// content's unless it says `size_full`: both are `OverlayLayer`'s.
#[gpui_kit::test]
fn a_cached_dialog_dims_the_panes(cx: &mut TestAppContext) {
    const MODULE: &str = "root-scrim-view";
    let (app, _, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    for overlay in [Overlay::Spotlight, Overlay::Approve] {
        super::tests::show(&view, Some(overlay), &mut native);
        // a miss (the dialog opened), then hits
        for _ in 0..3 {
            frame(&mut native);
            let quads = quads(&mut native);
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
            // the pane's box: a 1px border, taller than the dialog's rows
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

/// A toast drawn cached (no reader on) sits at the window's foot: its layer
/// is a layout root of its own, and says `size_full` so its `bottom` is the
/// window's.
#[gpui_kit::test]
fn a_cached_toast_sits_at_the_foot(cx: &mut TestAppContext) {
    let (app, _, _, mut native) = console(cx);
    set_motion(&app, false, &mut native);
    toast(&app, "Saved", &mut native);
    for _ in 0..3 {
        frame(&mut native);
        let scale = native.update(|window, _| window.scale_factor());
        let toast = quads(&mut native)
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
