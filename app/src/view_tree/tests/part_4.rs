/// A room column's refusal shape: a bordered, washed notice holding
/// one long wrapped line, then a Fill space, then the composer. The sidebar
/// beside it must keep its surface and rule, and the composer must sit at
/// the bottom of the column.
#[gpui_kit::test]
fn a_wrapped_notice_neither_starves_its_column_nor_its_neighbours_paint(
    cx: &mut gpui_kit::TestAppContext,
) {
    use view_wire::kit;
    cx.update(gpui_kit::init);
    let refusal = "Couldn’t read this room: indexer: view: unknown field `viewer_handles`, \
         expected one of `channel_id`, `before_seq`, `limit` at line 1 column 118";
    let room = kit::sized(
        kit::column(
            "room",
            [
                kit::divider("header-rule"),
                kit::notice(
                    "error",
                    kit::wrapping(kit::text("error-text", refusal)),
                    kit::Tone::Danger,
                ),
                kit::space(None, Some(wire::Length::Fill)),
                kit::sized(
                    kit::container("composer", wire::Node::empty()),
                    Some(wire::Length::Fill),
                    Some(wire::Length::Fixed(60.)),
                ),
            ],
        ),
        Some(wire::Length::Fill),
        Some(wire::Length::Fill),
    );
    let room = kit::sized(
        kit::row(
            "workspace",
            [
                kit::pane("sidebar", wire::Node::empty(), wire::Length::Fixed(236.)),
                kit::vertical_divider("sidebar-resize"),
                room,
            ],
        ),
        Some(wire::Length::Fill),
        Some(wire::Length::Fill),
    );
    // A room's real root: a viewport sensor around the press area.
    let root = wire::Node::Sensor {
        key: "viewport".into(),
        reset: None,
        on_show: None,
        on_resize: Some(1),
        on_hide: None,
        anticipate: None,
        delay: None,
        child: Box::new(wire::Node::MouseArea {
            key: "press-area".into(),
            role: None,
            label: None,
            expanded: None,
            selected: None,
            checked: None,
            on_press: Some(2),
            on_release: None,
            on_double_click: None,
            on_right_press: None,
            on_right_release: None,
            on_middle_press: None,
            on_middle_release: None,
            on_enter: None,
            on_exit: None,
            on_move: None,
            on_press_at: None,
            on_scroll: None,
            content: Box::new(room),
        }),
    };
    // Mounted the way a module seat mounts a guest: a cached view under a
    // full-size div, not as the window root (which gpui stretches).
    struct Seat(Entity<ViewTree>);
    impl Render for Seat {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.0
                    .clone()
                    .cached(gpui_kit::StyleRefinement::default().size_full()),
            )
        }
    }
    let window = cx.open_window(size(px(800.), px(600.)), |_, cx| {
        Seat(cx.new(|_| ViewTree::new(root)))
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    let bounds = |key: &str| {
        tree.read_with(&native, |tree, _| tree.measured_bounds(key))
            .unwrap_or_else(|| panic!("{key} was measured"))
    };
    let (sidebar, error, composer) = (bounds("sidebar"), bounds("error"), bounds("composer"));
    assert_eq!(sidebar.size, size(px(236.), px(600.)), "{sidebar:?}");
    assert!(
        error.size.height < px(200.) && error.size.height > px(20.),
        "the notice wraps to a few lines: {error:?}"
    );
    assert_eq!(
        composer.origin.y,
        px(540.),
        "the Fill space pushes the composer to the bottom: {composer:?}"
    );
    native.update(|window, cx| {
        window.render_frame(cx);
        let quads = window.painted_quads();
        let scale = window.scale_factor();
        let surface = |w: f32, h: f32| {
            quads
                .iter()
                .filter(|quad| {
                    quad.bounds.size.width.as_f32() == w * scale
                        && quad.bounds.size.height.as_f32() == h * scale
                })
                .count()
        };
        assert_eq!(surface(236., 600.), 1, "the sidebar surface: {quads:?}");
        let rules = quads
            .iter()
            .filter(|quad| quad.bounds.size.height.as_f32() == 600. * scale)
            .filter(|quad| quad.bounds.size.width.as_f32() <= scale)
            .count();
        assert_eq!(rules, 1, "the sidebar-resize rule: {quads:?}");
    });
}

#[gpui_kit::test]
fn a_float_modal_uses_viewport_coordinates_and_one_surface(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let card = wire::kit::sized(
        wire::kit::container("float-card", wire::Node::empty()),
        Some(wire::Length::Fixed(40.)),
        Some(wire::Length::Fixed(20.)),
    );
    let root = wire::Node::Overlay {
        key: "float-overlay".into(),
        label: Some("Context menu".into()),
        padding: 30.,
        backdrop: wire::Rgba([0.; 4]),
        align_x: wire::AlignX::Right,
        align_y: wire::AlignY::Bottom,
        on_dismiss: None,
        children: vec![
            wire::kit::spacer(),
            wire::Node::Float {
                key: "float".into(),
                x: 37.,
                y: 29.,
                scale: 1.,
                shadow: Default::default(),
                radius: None,
                content: Box::new(card),
            },
        ],
    };
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let card = tree
        .read_with(&native, |tree, _| tree.measured_bounds("float-card"))
        .expect("the floated card was measured");
    assert_eq!(card.origin, point(px(37.), px(29.)));
    native.update(|window, cx| {
        window.render_frame(cx);
        let quads = window.painted_quads();
        let scale = window.scale_factor();
        let card_surfaces = quads
            .iter()
            .filter(|quad| {
                quad.bounds.size.width.as_f32() == 40. * scale
                    && quad.bounds.size.height.as_f32() == 20. * scale
            })
            .count();
        assert_eq!(
            card_surfaces, 1,
            "the Float owns the card surface: {quads:?}"
        );
    });
}
