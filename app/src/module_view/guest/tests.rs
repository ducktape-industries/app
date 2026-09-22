use super::*;

fn tooltip_route(request: u32) -> wire::Node {
    let mut interactivity = wire::Interactivity::default();
    interactivity.tooltip = Some(wire::Tooltip {
        request,
        content: None,
        hoverable: false,
        delay_ms: 250,
    });
    wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: Default::default(),
        interactivity,
        children: Vec::new(),
    })
}

fn rich_tooltip_route(request: u32) -> wire::Node {
    wire::Node::RichText {
        id: Some(wire::ElementIdWire::Name("rich".into())),
        style: Default::default(),
        text: "alpha beta".into(),
        runs: wire::RichTextRuns::default(),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: Some(wire::RichTextTooltip {
            request,
            character_index: None,
            content: None,
        }),
    }
}

fn label(guest: &Guest) -> (String, u32) {
    match guest.frame.root.as_ref().expect("a tree") {
        wire::Node::Button {
            content: wire::ButtonContent::Label(label),
            on_press: Some(press),
            ..
        } => (label.clone(), *press),
        other => panic!("not the probe's button: {other:?}"),
    }
}

#[test]
fn tooltip_response_only_attaches_to_its_current_frame_route() {
    let mut held = Some(tooltip_route(7));
    let tip = wire::Node::Text(view_wire::TextNode {
        id: None,
        style: Default::default(),
        content: "Help".into(),
        heading: None,
        live: None,
    });
    let mut frame = wire::Frame {
        unchanged: true,
        tooltip_responses: vec![wire::TooltipResponse {
            request: 7,
            character_index: None,
            content: Some(Box::new(tip)),
        }],
        ..Default::default()
    };
    assert!(merge(&mut held, &mut frame).unwrap().0);
    let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = frame.root.unwrap()
    else {
        panic!("container")
    };
    assert!(interactivity.tooltip.unwrap().content.is_some());
}

#[test]
fn rich_tooltip_cache_is_replaced_for_the_exact_character_index() {
    let mut held = Some(rich_tooltip_route(8));
    let mut frame = wire::Frame {
        unchanged: true,
        tooltip_responses: vec![wire::TooltipResponse {
            request: 8,
            character_index: Some(6),
            content: Some(Box::new(wire::Node::empty())),
        }],
        ..Default::default()
    };
    assert!(merge(&mut held, &mut frame).unwrap().0);
    let wire::Node::RichText {
        tooltip: Some(tooltip),
        ..
    } = frame.root.as_ref().unwrap()
    else {
        panic!("rich tooltip route")
    };
    assert_eq!(tooltip.character_index, Some(6));
    assert!(tooltip.content.is_some());

    held = frame.root.take();
    let mut frame = wire::Frame {
        unchanged: true,
        tooltip_responses: vec![wire::TooltipResponse {
            request: 8,
            character_index: Some(0),
            content: None,
        }],
        ..Default::default()
    };
    assert!(merge(&mut held, &mut frame).unwrap().0);
    let wire::Node::RichText {
        tooltip: Some(tooltip),
        ..
    } = frame.root.unwrap()
    else {
        panic!("rich tooltip route")
    };
    assert_eq!(tooltip.character_index, Some(0));
    assert!(tooltip.content.is_none());
}

#[test]
fn tooltip_response_rejects_duplicate_authored_routes_before_mutating() {
    let mut held = Some(wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: Default::default(),
        interactivity: Default::default(),
        children: vec![tooltip_route(7), tooltip_route(7)],
    }));
    let mut frame = wire::Frame {
        unchanged: true,
        tooltip_responses: vec![wire::TooltipResponse {
            request: 7,
            character_index: None,
            content: Some(Box::new(wire::Node::empty())),
        }],
        ..Default::default()
    };
    assert_eq!(
        merge(&mut held, &mut frame),
        Err("duplicate tooltip request route")
    );
    let mut populated = 0;
    held.unwrap().for_each_mut(&mut |node| {
        if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = node
            && interactivity
                .tooltip
                .as_ref()
                .is_some_and(|tooltip| tooltip.content.is_some())
        {
            populated += 1;
        }
    });
    assert_eq!(populated, 0);
}

#[test]
fn duplicate_tooltip_responses_are_rejected_before_mutating() {
    let mut held = Some(tooltip_route(7));
    let response = || wire::TooltipResponse {
        request: 7,
        character_index: None,
        content: Some(Box::new(wire::Node::empty())),
    };
    let mut frame = wire::Frame {
        unchanged: true,
        tooltip_responses: vec![response(), response()],
        ..Default::default()
    };
    assert_eq!(
        merge(&mut held, &mut frame),
        Err("duplicate tooltip response request")
    );
    let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = held.unwrap()
    else {
        panic!("tooltip route")
    };
    assert!(interactivity.tooltip.unwrap().content.is_none());
}

#[test]
fn tooltip_response_is_sanitized_against_the_combined_held_tree_budget() {
    let mut children = vec![tooltip_route(7)];
    children.extend((0..wire::MAX_NODES - 2).map(|_| wire::Node::empty()));
    let mut held = Some(wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: Default::default(),
        interactivity: Default::default(),
        children,
    }));
    let response = wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: Default::default(),
        interactivity: Default::default(),
        children: (0..wire::MAX_NODES).map(|_| wire::Node::empty()).collect(),
    });
    let mut frame = wire::Frame {
        unchanged: true,
        tooltip_responses: vec![wire::TooltipResponse {
            request: 7,
            character_index: None,
            content: Some(Box::new(response)),
        }],
        ..Default::default()
    };
    assert!(merge(&mut held, &mut frame).unwrap().0);
    assert!(frame.root.unwrap().count() <= wire::MAX_NODES);
}

/// Every export of a real view, through guest memory. The bytes are
/// view-guest's `exported_view` example built for wasm32 by modules'
/// `make view-wasm-check`, named by `DUCKTAPE_VIEW_PROBE`.
#[test]
#[ignore = "needs DUCKTAPE_VIEW_PROBE=<exported_view.wasm>"]
fn a_core_module_view_inits_ticks_snapshots_and_restores() {
    let path = std::env::var("DUCKTAPE_VIEW_PROBE").expect("DUCKTAPE_VIEW_PROBE");
    let bytes = std::fs::read(path).expect("probe bytes");
    let mut guest = Guest::from_bytes("probe", &bytes, "probe").expect("loads");
    assert_eq!(guest.name, "Exported");
    guest.tick();
    assert_eq!(guest.fault, None);
    let (text, press) = label(&guest);
    assert_eq!(text, "0");

    guest.pending.push(wire::Event::Message(press));
    guest.tick();
    assert_eq!(label(&guest).0, "1");

    let state = guest.snapshot().expect("settled");
    let code = Guest::compile(&bytes, "probe")
        .map_err(|f| f.to_string())
        .expect("compiles");
    let mut next = Guest::instantiate("probe", &code, "probe").expect("instantiates");
    assert!(matches!(
        next.restore(&state, "probe"),
        Ok(Restored::Carried)
    ));
    assert!(matches!(
        next.restore(b"not json", "probe"),
        Ok(Restored::Refused(_))
    ));
    next.tick();
    assert_eq!(next.fault, None);
    assert_eq!(label(&next).0, "1");
}
