//! The sidebar (SPEC §5): its rows as the desk stands, what a press on one
//! does, where its dot and its menus land. Its keys are `roving.rs`', its
//! audit `gate.rs`' rows, a dialog over it `root_tests.rs`'.
use super::*;
use crate::shell::PaneMessage;
use crate::shell::layers::tests::pane;
use crate::ui::layout::{EMPTY, HELP};

pub(super) fn sidebar(programs: &[&str], windows: &[(&'static str, Option<&str>)]) -> Seed {
    let mut seed = gate::desk();
    seed.prefs.layout = Some(crate::backend::Layout::Sidebar);
    seed.roster = crate::runtime::Roster::listing(programs);
    let mut layout = crate::ui::layout::Layout::default();
    for (program, title) in windows {
        layout.split(program);
        let instance = layout.panes[layout.focused].instance;
        layout.set_title(instance, title.map(Into::into));
    }
    layout.measure((1060., 800.));
    layout.settle();
    layout.initialized = true;
    seed.layout = Some(layout);
    seed
}

fn drawn(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    draw(window, cx);
    serde_json::to_value(crate::ax::snapshot(
        "shell",
        window.a11y_tree(),
        window,
        true,
    ))
    .unwrap()
}

fn instances(view: &Entity<WindowRoot>, native: &mut VisualTestContext) -> Vec<u64> {
    view.read_with(native, |view, cx| {
        view.layout(cx)
            .panes
            .iter()
            .map(|pane| pane.instance)
            .collect()
    })
}

fn front(view: &Entity<WindowRoot>, native: &mut VisualTestContext) -> u64 {
    view.read_with(native, |view, cx| {
        let layout = view.layout(cx);
        layout.panes[layout.focused].instance
    })
}

fn rows(nodes: &serde_json::Value) -> Vec<(String, String, bool)> {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["role"] == "Tab")
        .map(|node| {
            let selected = node["state"]
                .as_array()
                .is_some_and(|states| states.iter().any(|it| it == "selected"));
            (
                node["id"].as_str().unwrap().to_owned(),
                node["name"].as_str().unwrap().to_owned(),
                selected,
            )
        })
        .collect()
}

fn node<'a>(nodes: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {nodes}"))
}

fn bounds(nodes: &serde_json::Value, id: &str) -> [f32; 4] {
    let at = |n: usize| node(nodes, id)["bounds"][n].as_f64().unwrap() as f32;
    [at(0), at(1), at(2), at(3)]
}

fn middle(nodes: &serde_json::Value, id: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
    let [x0, y0, x1, y1] = bounds(nodes, id);
    gpui_kit::point(px((x0 + x1) / 2.), px((y0 + y1) / 2.))
}

#[gpui_kit::test]
fn the_sidebar_lists_each_program_and_the_windows_it_has_open(cx: &mut TestAppContext) {
    let seed = sidebar(
        &["side-nodes", "side-members", "side-chat", "side-forge"],
        &[
            ("side-chat", None),
            ("side-members", None),
            ("side-chat", Some("# general")),
            ("side-forge", Some("website")),
            (HELP, None),
            (EMPTY, None),
            ("side-chat", None),
        ],
    );
    let (view, mut native) = open(seed, cx);
    let at = instances(&view, &mut native);
    let row =
        |id: String, name: &str, selected: bool| (format!("shell:{id}"), name.to_owned(), selected);
    let nodes = native.update(drawn);
    assert_eq!(
        rows(&nodes),
        [
            row("rail/side-nodes".into(), "Side nodes · Loading", false),
            row("rail/side-members".into(), "Side members · Loading", false),
            row("rail/side-chat".into(), "Side chat · Loading", false),
            row(format!("rail/side-chat/{}", at[0]), "Side chat", false),
            row(format!("rail/side-chat/{}", at[2]), "# general", false),
            row(format!("rail/side-chat/{}", at[6]), "Side chat 2", true),
            row("rail/side-forge".into(), "Side forge · Loading", false),
            row(format!("rail/side-forge/{}", at[3]), "website", false),
            row(format!("rail-help/{}", at[4]), "Help", false),
            row(format!("rail-new/{}", at[5]), "New window", false),
        ]
    );
    let [.., last] = bounds(&nodes, &format!("shell:rail/side-forge/{}", at[3]));
    let [_, help, ..] = bounds(&nodes, &format!("shell:rail-help/{}", at[4]));
    assert_eq!(help - last, 17., "the hairline before Help");
    for id in [
        "rail-rows",
        "network-switcher",
        "rail-search",
        "rail-notifications",
        "rail-connection",
        "rail-account",
        "settings",
    ] {
        node(&nodes, &format!("shell:{id}"));
    }
    pane(&view, PaneMessage::Focus(1), &mut native);
    let nodes = native.update(drawn);
    assert!(
        rows(&nodes).contains(&row(
            "rail/side-members".into(),
            "Side members · Loading",
            true
        )),
        "{:?}",
        rows(&nodes)
    );
    pane(&view, PaneMessage::Close(6), &mut native);
    let chat = |nodes: &serde_json::Value| -> Vec<String> {
        rows(nodes)
            .into_iter()
            .filter(|(id, ..)| id.starts_with("shell:rail/side-chat"))
            .map(|(_, name, _)| name)
            .collect()
    };
    let nodes = native.update(drawn);
    assert_eq!(
        chat(&nodes),
        ["Side chat · Loading", "Side chat", "# general"]
    );
    pane(&view, PaneMessage::Close(2), &mut native);
    let nodes = native.update(drawn);
    assert_eq!(chat(&nodes), ["Side chat · Loading"]);
}

#[gpui_kit::test]
fn a_press_on_a_window_row_brings_that_window_forward(cx: &mut TestAppContext) {
    let seed = sidebar(
        &["side-press", "side-front"],
        &[
            ("side-press", Some("# general")),
            ("side-press", Some("# design")),
            ("side-press", Some("# random")),
            ("side-front", None),
        ],
    );
    let (view, mut native) = open(seed, cx);
    let at = instances(&view, &mut native);
    pane(&view, PaneMessage::Close(0), &mut native);
    let nodes = native.update(drawn);
    let random = format!("shell:rail/side-press/{}", at[2]);
    native.simulate_click(middle(&nodes, &random), gpui_kit::Modifiers::none());
    assert_eq!(front(&view, &mut native), at[2], "# random came forward");
    let nodes = native.update(drawn);
    assert!(rows(&nodes).contains(&(random, "# random".into(), true)));
}

#[gpui_kit::test]
fn a_title_given_later_lists_the_window_under_its_program(cx: &mut TestAppContext) {
    let (view, mut native) = open(sidebar(&["side-named"], &[("side-named", None)]), cx);
    let [instance] = instances(&view, &mut native)[..] else {
        panic!("one window")
    };
    let named = |native: &mut VisualTestContext| -> Vec<(String, String)> {
        rows(&native.update(drawn))
            .into_iter()
            .map(|(id, name, _)| (id, name))
            .collect()
    };
    let program = || {
        (
            "shell:rail/side-named".to_owned(),
            "Side named · Loading".to_owned(),
        )
    };
    assert_eq!(named(&mut native), [program()]);
    let desk = view.read_with(&native, |view, _| view.desk.clone());
    let title = |title: &str, native: &mut VisualTestContext| {
        desk.update(native, |desk, cx| {
            desk.set_title(instance, title.into(), cx)
        })
    };
    title("# general", &mut native);
    assert_eq!(
        named(&mut native),
        [
            program(),
            (
                format!("shell:rail/side-named/{instance}"),
                "# general".into()
            )
        ]
    );
    title("", &mut native);
    assert_eq!(named(&mut native), [program()]);
}

#[gpui_kit::test]
fn a_press_on_a_window_row_while_filled_fills_that_window(cx: &mut TestAppContext) {
    let mut seed = sidebar(
        &["side-fill"],
        &[
            ("side-fill", Some("# general")),
            ("side-fill", Some("# design")),
        ],
    );
    seed.layout.as_mut().unwrap().toggle_fill();
    let (view, mut native) = open(seed, cx);
    let at = instances(&view, &mut native);
    let nodes = native.update(drawn);
    assert_eq!(bounds(&nodes, "shell:pane/1"), [220., 0., 1280., 800.]);
    let general = format!("shell:rail/side-fill/{}", at[0]);
    native.simulate_click(middle(&nodes, &general), gpui_kit::Modifiers::none());
    assert_eq!(front(&view, &mut native), at[0], "# general came forward");
    assert!(view.read_with(&native, |view, cx| view.layout(cx).filled));
    let nodes = native.update(drawn);
    assert_eq!(bounds(&nodes, "shell:pane/0"), [220., 0., 1280., 800.]);
    assert!(rows(&nodes).contains(&(general, "# general".into(), true)));
}

#[gpui_kit::test]
fn a_popped_out_window_stays_listed_and_its_row_brings_it_forward(cx: &mut TestAppContext) {
    let seed = sidebar(
        &["side-pop", "side-stay"],
        &[("side-stay", None), ("side-pop", None)],
    );
    let (app, key, view, mut native) = crate::shell::layers::tests::open_console(seed, cx);
    let [_, popped] = instances(&view, &mut native)[..] else {
        panic!("two windows")
    };
    let (_, handle, _) = crate::shell::layers::tests::pop_out(&app, key, &view, &mut native);
    let in_front = |native: &mut VisualTestContext| {
        native.update(|_, cx| {
            handle
                .update(cx, |_, window, _| window.is_window_active())
                .unwrap()
        })
    };
    activate(&mut native);
    assert!(!in_front(&mut native), "the console is in front");
    let nodes = native.update(drawn);
    let row = format!("shell:rail/side-pop/{popped}");
    let listed =
        |id: &str, name: &str, selected: bool| (format!("shell:{id}"), name.to_owned(), selected);
    assert_eq!(
        rows(&nodes),
        [
            listed("rail/side-pop", "Side pop · Loading", false),
            listed(&format!("rail/side-pop/{popped}"), "Side pop", false),
            listed("rail/side-stay", "Side stay · Loading", true),
        ]
    );
    assert_eq!(node(&nodes, &row)["description"], "In a window of its own");
    let errors = super::gate::errors(&mut native, "sidebar-popped", false);
    assert!(errors.is_empty(), "{errors:#?}");

    let nodes = native.update(drawn);
    native.simulate_click(middle(&nodes, &row), gpui_kit::Modifiers::none());
    native.run_until_parked();
    assert!(in_front(&mut native), "the pop-out came forward");
    let nodes = native.update(drawn);
    assert!(
        rows(&nodes).contains(&(row.clone(), "Side pop".into(), true)),
        "{:?}",
        rows(&nodes)
    );

    activate(&mut native);
    let nodes = native.update(drawn);
    native.simulate_click(
        middle(&nodes, "shell:rail/side-pop"),
        gpui_kit::Modifiers::none(),
    );
    native.run_until_parked();
    assert!(in_front(&mut native), "its program brought it forward");
    let programs: Vec<&str> = view.read_with(&native, |view, cx| {
        view.layout(cx)
            .panes
            .iter()
            .map(|pane| pane.program)
            .collect()
    });
    assert_eq!(
        programs,
        ["side-stay"],
        "the program opened on the desk again"
    );
}

#[gpui_kit::test]
fn the_plus_on_a_program_row_opens_another_window_of_it(cx: &mut TestAppContext) {
    let seed = sidebar(&["side-plus", "side-other"], &[("side-plus", None)]);
    let (view, mut native) = open(seed, cx);
    let plus = "shell:rail/side-plus/split";
    let nodes = native.update(drawn);
    assert!(
        !ids(&nodes).iter().any(|id| id == plus),
        "a + before the pointer came"
    );
    native.simulate_mouse_move(
        middle(&nodes, "shell:rail/side-plus"),
        None,
        gpui_kit::Modifiers::none(),
    );
    let nodes = native.update(drawn);
    assert_eq!(node(&nodes, plus)["name"], "Open another Side plus window");
    let errors = super::gate::errors(&mut native, "sidebar-plus", false);
    assert!(errors.is_empty(), "{errors:#?}");
    for row in ["shell:rail/side-other", "shell:rail/side-plus"] {
        native.simulate_mouse_move(middle(&nodes, row), None, gpui_kit::Modifiers::none());
    }
    let nodes = native.update(drawn);
    native.simulate_click(middle(&nodes, plus), gpui_kit::Modifiers::none());
    let programs: Vec<&str> = view.read_with(&native, |view, cx| {
        view.layout(cx)
            .panes
            .iter()
            .map(|pane| pane.program)
            .collect()
    });
    assert_eq!(programs, ["side-plus", "side-plus"]);
    let at = instances(&view, &mut native);
    assert_eq!(front(&view, &mut native), at[1], "the new window in front");
    let nodes = native.update(drawn);
    native.simulate_mouse_move(
        middle(&nodes, "shell:rail/side-other"),
        None,
        gpui_kit::Modifiers::none(),
    );
    let nodes = native.update(drawn);
    assert!(!ids(&nodes).iter().any(|id| id == plus), "the + stayed");
    native.simulate_mouse_move(
        middle(&nodes, "shell:rail/side-plus"),
        None,
        gpui_kit::Modifiers::none(),
    );
    native.update(drawn);
    let app = entities(&view, &mut native);
    let switch = |layout, native: &mut VisualTestContext| {
        app.prefs
            .update(native, |prefs, cx| prefs.set_layout(layout, cx));
        native.update(drawn);
    };
    switch(crate::backend::Layout::MenuBar, &mut native);
    let desk = gpui_kit::point(px(900.), px(500.));
    native.simulate_mouse_move(desk, None, gpui_kit::Modifiers::none());
    switch(crate::backend::Layout::Sidebar, &mut native);
    let nodes = native.update(drawn);
    assert!(!ids(&nodes).iter().any(|id| id == plus), "a + left over");
}

fn ids(nodes: &serde_json::Value) -> Vec<String> {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_owned))
        .collect()
}

#[gpui_kit::test]
fn the_desk_and_the_dot_follow_the_sidebar(cx: &mut TestAppContext) {
    use crate::backend::Layout;
    let (view, mut native) = open(sidebar(&[], &[]), cx);
    let placed = |native: &mut VisualTestContext| {
        native.update(drawn);
        native.update(|_, cx| {
            let root = view.read(cx);
            let windows = root.app.windows.read(cx);
            let dot = windows.own(root.key).unwrap().dot.read(cx);
            let dot = (*dot.get()).expect("the well was laid out");
            (root.layout(cx).desk, dot)
        })
    };
    let (desk, dot) = placed(&mut native);
    assert_eq!(
        desk,
        Some((1280. - 220., 800.)),
        "the desk beside the column"
    );
    let (x, y) = (f32::from(dot.origin.x), f32::from(dot.origin.y));
    assert!(x < 220. && y > 800. - 40., "the dot at {dot:?}");
    let app = entities(&view, &mut native);
    app.prefs.update(&mut native, |prefs, cx| {
        prefs.set_layout(Layout::MenuBar, cx)
    });
    let (desk, dot) = placed(&mut native);
    assert_eq!(desk, Some((1280., 800. - 36.)), "the desk under the bar");
    assert!(
        f32::from(dot.origin.y) < 36. && f32::from(dot.origin.x) > 220.,
        "the dot at {dot:?}"
    );
}

#[gpui_kit::test]
fn a_window_dragged_left_stops_at_the_sidebar(cx: &mut TestAppContext) {
    use crate::shell::panes_tests::drag;
    let seed = sidebar(
        &["side-left", "side-front"],
        &[("side-left", None), ("side-front", None)],
    );
    let (view, mut native) = open(seed, cx);
    let at = instances(&view, &mut native);
    let frame = |native: &mut VisualTestContext| {
        view.read_with(native, |view, cx| view.layout(cx).panes[0].frame.unwrap())
    };
    let start = frame(&mut native);
    let title = (220. + start.x + 100., start.y + 15.);
    drag(&mut native, title, (-500., title.1));
    let stopped = frame(&mut native);
    assert_eq!((stopped.x, stopped.y, stopped.w), (0., start.y, start.w));
    let nodes = native.update(drawn);
    assert_eq!(
        bounds(&nodes, "shell:pane/0")[0],
        220.,
        "at the column's edge"
    );
    pane(&view, PaneMessage::Focus(1), &mut native);
    let column = (218., stopped.y + stopped.h / 2.);
    drag(&mut native, column, (column.0 + 80., column.1));
    assert_eq!(
        frame(&mut native),
        stopped,
        "the column took hold of its edge"
    );
    assert_eq!(front(&view, &mut native), at[1], "the column raised it");
}

#[gpui_kit::test]
fn the_sidebars_menus_hang_beside_it(cx: &mut TestAppContext) {
    let [x0, y0, _, _] = {
        let (_view, mut native) = open((sidebar(&[], &[]), Overlay::Network), cx);
        let nodes = native.update(drawn);
        bounds(&nodes, "shell:network-menu")
    };
    assert_eq!((x0, y0), (224., 0.), "the network's menu");
    for (menu, card, button) in [
        (
            Popover::Notifications,
            "notifications",
            "rail-notifications",
        ),
        (Popover::Node, "node-status", "rail-connection"),
        (Popover::Account, "account-menu", "rail-account"),
    ] {
        let (_view, mut native) = open((sidebar(&[], &[]), Overlay::Menu(menu)), cx);
        let nodes = native.update(drawn);
        let [x0, _, _, y1] = bounds(&nodes, &format!("shell:{card}"));
        let [left, top, ..] = bounds(&nodes, &format!("shell:{button}"));
        assert_eq!(
            top,
            800. - 39.,
            "{button} in the footer, under its hairline"
        );
        assert_eq!((x0, y1), (left, 800. - 40. - 4.), "{card} over {button}");
    }
    for (menu, card) in [
        (Popover::Notifications, "notifications"),
        (Popover::Node, "node-status"),
        (Popover::Account, "account-menu"),
    ] {
        let (_view, mut native) = open((sidebar(&[], &[]), Overlay::Menu(menu)), cx);
        native.simulate_resize(size(px(1280.), px(160.)));
        let nodes = native.update(drawn);
        let [_, y0, _, y1] = bounds(&nodes, &format!("shell:{card}"));
        assert_eq!((y0, y1), (36. + 4., 160. - 40. - 4.), "{card}, short");
    }
}
