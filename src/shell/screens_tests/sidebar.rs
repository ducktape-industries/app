//! The sidebar (SPEC §5): its rows as the desk stands, what a press on one
//! does, where its dot and its menus land. Its keys are `roving.rs`', its
//! audit `gate.rs`' rows, a dialog over it `root_tests.rs`'.
use super::*;
use crate::shell::PaneMessage;
use crate::shell::layers::tests::pane;
use crate::ui::layout::{EMPTY, HELP};

/// A desk in the sidebar layout over `programs` (none drawn yet: each is
/// listed loading, by its id prettified), a window open for each of
/// `windows` (its program, its title) in that order, the last in front.
pub(super) fn sidebar(programs: &[&str], windows: &[(&'static str, Option<&str>)]) -> Seed {
    let mut seed = gate::desk();
    seed.prefs.layout = Some(crate::backend::Layout::Sidebar);
    seed.roster = crate::runtime::Roster::listing(programs);
    let mut layout = crate::ui::layout::Layout::default();
    for (module, title) in windows {
        layout.split(module);
        let instance = layout.panes[layout.focused].instance;
        layout.set_title(instance, title.map(Into::into));
    }
    layout.measure((1060., 800.));
    layout.settle();
    layout.initialized = true;
    seed.layout = Some(layout);
    seed
}

/// [`draw`], with every node's bounds.
fn drawn(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    draw(window, cx);
    serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap()
}

/// The desk's windows' instances, in desk order.
fn instances(view: &Entity<WindowRoot>, native: &mut VisualTestContext) -> Vec<u64> {
    view.read_with(native, |view, cx| {
        view.layout(cx)
            .panes
            .iter()
            .map(|pane| pane.instance)
            .collect()
    })
}

/// The window in front's instance.
fn front(view: &Entity<WindowRoot>, native: &mut VisualTestContext) -> u64 {
    view.read_with(native, |view, cx| {
        let layout = view.layout(cx);
        layout.panes[layout.focused].instance
    })
}

/// The sidebar list's rows, in order: each one's id, name, and whether it
/// is selected.
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

/// A node's bounds: `[x0, y0, x1, y1]`.
fn bounds(nodes: &serde_json::Value, id: &str) -> [f32; 4] {
    let at = |n: usize| node(nodes, id)["bounds"][n].as_f64().unwrap() as f32;
    [at(0), at(1), at(2), at(3)]
}

/// The middle of a node.
fn middle(nodes: &serde_json::Value, id: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
    let [x0, y0, x1, y1] = bounds(nodes, id);
    gpui_kit::point(px((x0 + x1) / 2.), px((y0 + y1) / 2.))
}

/// SPEC §5's rows: each program in roster order; under one, its windows in
/// desk order when it has two, or one with a title (a title, else the
/// program's name counted from the second untitled one); a program with no
/// window or one untitled window is its row alone, the second standing for
/// that window (selected while it is in front); Help and empty windows after
/// them. The bar's ids are the sidebar's. Closing windows folds a program
/// back (Review Focus 4: one titled, one not, then the titled one closed).
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
    // Help and the empty window after a hairline: 1px, 8px either side
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
    // Members' one window in front: its program's row stands for it
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
    // one untitled and one titled Chat window: both listed
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
    // the titled one closed: Chat folds back to its row alone
    pane(&view, PaneMessage::Close(2), &mut native);
    let nodes = native.update(drawn);
    assert_eq!(chat(&nodes), ["Side chat · Loading"]);
}

/// A press on a window's row brings that window to the front, found by its
/// instance at the press: a window closed before it moved its place, and
/// the program's first window is another.
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

/// Under the pointer a program's row shows a `+`, and a press on it opens
/// another window of the program, in front; away from the row it is gone.
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
    // shown, the + passes the audit; its walk leaves no row hovered, so
    // the pointer comes back to this one
    let errors = super::gate::errors(&mut native, "sidebar-plus", false);
    assert!(errors.is_empty(), "{errors:#?}");
    for row in ["shell:rail/side-other", "shell:rail/side-plus"] {
        native.simulate_mouse_move(middle(&nodes, row), None, gpui_kit::Modifiers::none());
    }
    let nodes = native.update(drawn);
    native.simulate_click(middle(&nodes, plus), gpui_kit::Modifiers::none());
    let modules: Vec<&str> = view.read_with(&native, |view, cx| {
        view.layout(cx)
            .panes
            .iter()
            .map(|pane| pane.module)
            .collect()
    });
    assert_eq!(modules, ["side-plus", "side-plus"]);
    let at = instances(&view, &mut native);
    assert_eq!(front(&view, &mut native), at[1], "the new window in front");
    // the pointer on another row: this one's + goes
    let nodes = native.update(drawn);
    native.simulate_mouse_move(
        middle(&nodes, "shell:rail/side-other"),
        None,
        gpui_kit::Modifiers::none(),
    );
    let nodes = native.update(drawn);
    assert!(!ids(&nodes).iter().any(|id| id == plus), "the + stayed");
}

/// The ids in a snapshot.
fn ids(nodes: &serde_json::Value) -> Vec<String> {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_owned))
        .collect()
}

/// The desk sits right of the sidebar, the window high, and the node's
/// breath is drawn over the well the sidebar's footer laid out; the layout
/// switched back, the desk is under the bar and the breath over its well.
#[gpui_kit::test]
fn the_desk_and_the_dot_follow_the_sidebar(cx: &mut TestAppContext) {
    use crate::backend::Layout;
    let (view, mut native) = open(sidebar(&[], &[]), cx);
    // the desk as measured, and where the dot sits
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

/// In the sidebar a menu opens beside the column: the network's 4px right
/// of it at its row's top; the bell's, the node's and the account's upward
/// from their buttons in the footer, their foot 4px over its hairline, and
/// no taller than what is left over it.
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
    // a window too short for the account's menu: it scrolls in what is
    // left over the footer, 4px inside the window's top
    let (_view, mut native) = open((sidebar(&[], &[]), Overlay::Menu(Popover::Account)), cx);
    native.simulate_resize(size(px(1280.), px(280.)));
    let nodes = native.update(drawn);
    let [_, y0, _, y1] = bounds(&nodes, "shell:account-menu");
    assert_eq!((y0, y1), (4., 280. - 40. - 4.), "the account's menu, short");
}
