//! The window's layers, counted: each draws for what it observes and for
//! nothing else (P1, P6 in the app), and the root lays them out as the
//! window's kind and screen say (docs/perf.md).
use super::tests::open_console;
use crate::runtime::WindowKey;
use crate::shell::entities::{Overlay, Popover};
use crate::shell::panes_tests::{console, draw, settle, window_count};
use crate::shell::{Desktop, Message, PaneMessage, WindowKind, WindowRoot};
use crate::ui::test_support::status;
use gpui_kit::{AnyWindowHandle, Entity, Styled as _, TestAppContext, VisualTestContext, px};
use view_wire as wire;

/// A frame as the platform delivers one: what asked for it runs, then
/// whatever that dirtied draws.
fn frame(native: &mut VisualTestContext) {
    native.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    native.run_until_parked();
}

fn line(text: &str) -> wire::Node {
    wire::Node::RichText {
        id: Some(wire::ElementIdWire::Name("line".into())),
        style: gpui_kit::div().h(px(20.)).style().clone(),
        text: text.into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    }
}

fn tree_renders(module: &str) -> u64 {
    crate::perf::snapshot(false)["views"][module]["renders"]
        .as_u64()
        .unwrap_or_else(|| panic!("{module} counts its renders"))
}

/// The ids in a snapshot, in the tree's order.
fn ids(nodes: &serde_json::Value) -> Vec<String> {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_owned))
        .collect()
}

/// The node's breath is a view of its own: with motion on, each of its
/// pulses draws the dot and the window's root, and the bar (cached, with
/// its well empty) not at all.
#[gpui_kit::test]
fn a_pulse_re_renders_the_dot_and_not_the_chrome(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (model, key, _, mut native) = console(cx);
    native.update(|_, cx| cx.set_reduce_motion(false));
    native.update(|window, _| window.activate_window());
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::SetMotion(true), cx);
        model.dispatch(Message::StatusPushed(status(7)), cx);
    });
    // frames, not `draw`: the door's tree (`activate_a11y`) draws every
    // layer uncached until s15
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

/// A seat that wakes (its view lands again) draws its own tree and leaves
/// the bar cached.
#[gpui_kit::test]
fn a_pane_wake_leaves_the_chrome_alone(cx: &mut TestAppContext) {
    const MODULE: &str = "root-wake-view";
    let _on = crate::perf::on_for_test();
    let (model, key, _, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("first"));
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::SetMotion(false), cx);
        model.dispatch(Message::Pane(key, PaneMessage::Select(MODULE)), cx);
    });
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
    let (model, key, _, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::SetMotion(false), cx);
        model.dispatch(Message::Pane(key, PaneMessage::Select(MODULE)), cx);
    });
    for _ in 0..8 {
        frame(&mut native);
    }
    let (chrome, toast, tree) = (
        window_count(key, "renders.chrome"),
        window_count(key, "renders.toast"),
        tree_renders(MODULE),
    );
    assert!(chrome > 0 && toast > 0 && tree > 0, "every layer drew");
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::ShowToast("Saved".into()), cx)
    });
    frame(&mut native);
    assert_eq!(
        window_count(key, "renders.toast"),
        toast + 1,
        "the toast drew its layer other than once"
    );
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome,
        "a toast drew the bar"
    );
    assert_eq!(tree_renders(MODULE), tree, "a toast drew a pane's tree");
    // the door's tree after the counts: it draws every layer uncached
    let nodes = native.update(draw);
    assert!(
        ids(&nodes).iter().any(|id| id == "console:toast"),
        "no toast up: {nodes}"
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
    let (model, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::SetMotion(false), cx);
        model.dispatch(Message::Pane(key, PaneMessage::Select(MODULE)), cx);
    });
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
    let spotlight = model.read_with(&native, |model, _| {
        model.entities.by_window[&key].spotlight.clone()
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

/// The console's front pane popped out to a window of its own: its key
/// and handle.
fn pop_out(
    model: &Entity<Desktop>,
    key: WindowKey,
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
) -> (WindowKey, AnyWindowHandle) {
    let index = native.update(|_, cx| view.read(cx).layout(cx).focused);
    model.update(native, |model, cx| {
        model.dispatch(
            Message::Pane(key, PaneMessage::PopOut { index, at: None }),
            cx,
        )
    });
    // the model put the pane in a window of its own and asked the shell to
    // open it; the test has no command loop, so it opens it the same way
    native.update(|_, cx| {
        let (&popped, own) = model
            .read(cx)
            .state
            .layouts
            .iter()
            .find(|(candidate, _)| **candidate != key)
            .expect("the pane left for a window of its own");
        let kind = WindowKind::View {
            module: own.panes[0].module,
        };
        model.update(cx, |model, cx| {
            model.open_window(
                popped,
                kind,
                futures::channel::oneshot::channel().0,
                None,
                cx,
            )
        });
    });
    native.run_until_parked();
    native.update(|_, cx| {
        let model = model.read(cx);
        let (&key, _) = model
            .views
            .iter()
            .find(|(candidate, _)| **candidate != key)
            .expect("popout opened a view window");
        (key, model.windows[&key])
    })
}

/// A pane popped out to a window of its own: that window's root draws the
/// panes and the footer, and no bar.
#[gpui_kit::test]
fn a_pop_out_draws_panes_and_footer_and_no_bar(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (model, key, view, mut native) = console(cx);
    settle(&mut native);
    let (popped_key, popped_handle) = pop_out(&model, key, &view, &mut native);
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
    let (model, key, view, mut native) = console(cx);
    settle(&mut native);
    let (popped_key, popped_handle) = pop_out(&model, key, &view, &mut native);
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
            *model.read(cx).entities.by_window[&key]
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
/// each screen change draws it once, and no bar until the desk.
#[gpui_kit::test]
fn the_launcher_is_the_console_root_before_the_desk(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (mut state, _) = crate::Ducktape::boot();
    state.center = Default::default();
    state.roster = Default::default();
    state.key_exists = true;
    let (model, key, _, mut native) = open_console(state, cx);
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
    model.update(&mut native, |model, cx| {
        model.state.stage = crate::Stage::Unlock(Default::default());
        model.bridge(false, cx);
        cx.notify();
    });
    frame(&mut native);
    assert_eq!(
        window_count(key, "renders.launcher"),
        launcher + 1,
        "a screen change drew the launcher other than once"
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
    model.update(&mut native, |model, cx| {
        model.state.stage = crate::Stage::Desk;
        model.bridge(false, cx);
        cx.notify();
    });
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

/// A toast paints over an open menu, as it did before it was a layer: its
/// node comes after the menu's in the tree.
#[gpui_kit::test]
fn a_toast_paints_over_an_open_menu(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = console(cx);
    super::tests::show(&view, Some(Overlay::Menu(Popover::Node)), &mut native);
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::ShowToast("Saved".into()), cx);
    });
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
    let (model, key, _, mut native) = console(cx);
    crate::runtime::seat_for_test(MODULE, 400);
    native.update(|window, _| window.activate_window());
    settle(&mut native);
    let front = |native: &mut VisualTestContext| {
        native.update(|_, cx| model.read(cx).state.center.lock().front)
    };
    assert_ne!(front(&mut native), Some((key, MODULE)));
    // one message, no frame: the observer alone tells the centre
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::Pane(key, PaneMessage::Select(MODULE)), cx);
    });
    native.run_until_parked();
    assert_eq!(front(&mut native), Some((key, MODULE)));
}

/// Raising a window that is already active still times the switch to the
/// frame after the one it asks for: the timer closes and lands a sample.
#[gpui_kit::test]
fn raising_an_active_window_closes_its_switch_timer(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (model, key, _, mut native) = console(cx);
    native.update(|window, _| window.activate_window());
    settle(&mut native);
    let samples = |native: &mut VisualTestContext| {
        native.run_until_parked();
        crate::perf::snapshot(false)["windows"][key.0.to_string()]["switch"]["n"]
            .as_u64()
            .unwrap_or(0)
    };
    let before = samples(&mut native);
    model.update(&mut native, |model, cx| {
        model.execute(crate::shell::NativeCommand::Raise(key), cx)
    });
    // the frame the raise asks for, then the one after it
    frame(&mut native);
    frame(&mut native);
    assert_eq!(
        samples(&mut native),
        before + 1,
        "the switch timer stayed open"
    );
}
