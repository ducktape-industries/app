use super::*;

fn tooltip_route(request: u32) -> wire::Node {
    primitive_tooltip_route("container", request)
}

fn primitive_tooltip_route(kind: &str, request: u32) -> wire::Node {
    let interactivity = wire::Interactivity {
        tooltip: Some(wire::Tooltip {
            request,
            content: None,
            hoverable: false,
            delay_ms: 250,
        }),
        ..Default::default()
    };
    match kind {
        "container" => wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: Default::default(),
            interactivity,
            children: Vec::new(),
        }),
        "uniform-list" => wire::Node::UniformList {
            id: wire::ElementIdWire::Name("list".into()),
            path: vec![wire::ElementIdWire::Name("list".into())],
            route: 90,
            style: Default::default(),
            interactivity,
            count: 0,
            measure_index: 0,
            sizing: Default::default(),
            horizontal_sizing: Default::default(),
            y_flipped: false,
            scroll_request: None,
            indices: Vec::new(),
            children: Vec::new(),
        },
        "image" => wire::Node::Image {
            id: Some(wire::ElementIdWire::Name("image".into())),
            hash: 0,
            data: None,
            label: None,
            image_style: wire::ImageStyle {
                grayscale: false,
                object_fit: wire::ImageObjectFit::Contain,
            },
            loading: false,
            fallback: false,
            state_children: Vec::new(),
            style: Default::default(),
            interactivity,
        },
        "svg" => wire::Node::Svg {
            id: Some(wire::ElementIdWire::Name("svg".into())),
            source: wire::SvgSource::None,
            transformation: wire::SvgTransformation {
                scale: [1.0, 1.0],
                translate: [0.0, 0.0],
                rotate: 0.0,
            },
            label: None,
            style: Default::default(),
            interactivity,
        },
        _ => unreachable!(),
    }
}

fn primitive_tooltip(node: &wire::Node) -> &wire::Tooltip {
    match node {
        wire::Node::Container(view_wire::ContainerNode { interactivity, .. })
        | wire::Node::UniformList { interactivity, .. }
        | wire::Node::Image { interactivity, .. }
        | wire::Node::Svg { interactivity, .. } => interactivity.tooltip.as_ref().unwrap(),
        _ => unreachable!(),
    }
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
        tooltip: Some(wire::TooltipResponse {
            request,
            character_index: None,
            content: None,
        }),
    }
}

fn label(guest: &Guest) -> (String, u32) {
    match guest.frame.root.as_ref().expect("a tree") {
        wire::Node::Container(wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name(id)),
            interactivity,
            children,
            ..
        }) if id == "press" => {
            let [wire::Node::Text(wire::TextNode { content, .. })] = children.as_slice() else {
                panic!("the probe's click element must contain its counter text");
            };
            (
                content.clone(),
                interactivity.on_click.expect("click route"),
            )
        }
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
fn tooltip_responses_merge_into_every_interactive_primitive_kind() {
    for kind in ["container", "uniform-list", "image", "svg"] {
        let mut held = Some(primitive_tooltip_route(kind, 7));
        let mut frame = wire::Frame {
            unchanged: true,
            tooltip_responses: vec![wire::TooltipResponse {
                request: 7,
                character_index: None,
                content: Some(Box::new(wire::Node::empty())),
            }],
            ..Default::default()
        };
        assert!(merge(&mut held, &mut frame).unwrap().0, "{kind}");
        assert!(
            primitive_tooltip(frame.root.as_ref().unwrap())
                .content
                .is_some(),
            "{kind}"
        );
    }
}

#[test]
fn duplicate_tooltip_routes_across_primitive_kinds_are_refused_before_mutating() {
    let mut held = Some(wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: Default::default(),
        interactivity: Default::default(),
        children: vec![
            primitive_tooltip_route("uniform-list", 7),
            primitive_tooltip_route("image", 7),
            primitive_tooltip_route("svg", 7),
        ],
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
    assert!(
        held.unwrap()
            .children()
            .iter()
            .all(|node| primitive_tooltip(node).content.is_none())
    );
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

/// Every export of a real view, through guest memory and the swap a
/// deployment takes: the drawn view ticked, its state carried into a fresh
/// instance of the same code, and that instance's first tree drawn from
/// it. The bytes are view-guest's `exported_view` example built for wasm32
/// by modules' `make view-wasm-check`, named by `DUCKTAPE_VIEW_PROBE`.
#[test]
#[ignore = "needs DUCKTAPE_VIEW_PROBE=<exported_view.wasm>"]
fn a_ticked_view_survives_a_swap_with_its_state() {
    let path = std::env::var("DUCKTAPE_VIEW_PROBE").expect("DUCKTAPE_VIEW_PROBE");
    let bytes = std::fs::read(path).expect("probe bytes");
    let mut old = Guest::from_bytes("probe", &bytes, "probe").expect("loads");
    assert_eq!(old.name, "Exported");
    old.tick();
    old.ticks += 1;
    assert_eq!(old.fault, None);
    let (text, press) = label(&old);
    assert_eq!(text, "0");

    old.pending.push(wire::Event::Click {
        handler: press,
        event: wire::click::Click::Keyboard {
            button: wire::click::KeyboardButton::Enter,
            bounds: Default::default(),
        },
    });
    old.tick();
    old.ticks += 1;
    assert_eq!(label(&old).0, "1");
    let state = old.snapshot().expect("no trap").expect("settled");
    assert!(state[0] >= 0x80, "the state is a named MessagePack map");

    let alive = old.alive.clone();
    let mounted = Mounted::seat();
    mounted.lock().unwrap().slot = Slot::Ready(Box::new(old));
    let code = Guest::compile(&bytes, "probe")
        .map_err(|f| f.to_string())
        .expect("compiles");
    let fresh = Guest::instantiate("probe", &code, "probe").expect("instantiates");
    let mut ticks = 0;
    let mut timing = LoadTiming::default();
    let fresh = Guest::replacement(fresh, &alive, &mut ticks, &mounted, "probe", &mut timing)
        .expect("the replacement carries the state");
    assert_eq!(ticks, 2, "the count the snapshot was taken at");
    assert!(fresh.staged && fresh.fault.is_none());
    assert_eq!(label(&fresh).0, "1", "drawn from the state it was handed");

    // a state that is not the view's own is the guest's refusal, not a trap
    let mut next = Guest::instantiate("probe", &code, "probe").expect("instantiates");
    assert!(matches!(
        next.restore(b"not a snapshot", "probe"),
        Ok(Restored::Refused(_))
    ));
}

#[test]
fn a_view_built_against_another_wire_is_refused_at_load() {
    assert!(wire_id(wire::WIRE_ID).is_ok());
    let refused = wire_id("0").unwrap_err();
    assert!(
        refused.contains("wire 0;") && refused.contains(wire::WIRE_ID),
        "{refused}"
    );
}

/// Pages a view's memory holds for the swap tests: a full snapshot budget
/// plus its tag byte, with room to spare for the events and the frame.
const SWAP_PAGES: usize = wire::MAX_SNAPSHOT_BYTES / 65536 + 1;

/// A view in WAT whose `snapshot`, `restore` and `tick` are the bodies
/// given, each leaving the packed `i64` it answers with. Memory byte 0 is
/// a result's Ok tag and byte 1 a refusal's; `init` marks byte 2 so a test
/// sees whether it ran; `tick`'s frame sits at the top of memory, clear of
/// the buffer `alloc` hands out at 64.
fn wat_view(snapshot: &str, restore: &str, tick: Option<&[u8]>) -> Guest {
    let top = SWAP_PAGES * 65536;
    let tick_body = match tick {
        Some(frame) => format!(
            "i64.const {}",
            wire::abi::pack((top - frame.len()) as u32, frame.len() as u32)
        ),
        None => "unreachable".to_string(),
    };
    let module = wasmtime::Module::new(
        engine(),
        format!(
            r#"(module
            (memory (export "memory") {SWAP_PAGES})
            (data (i32.const 1) "\01")
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init") (i32.store8 (i32.const 2) (i32.const 1)))
            (func (export "tick") (param i32 i32) (result i64) {tick_body})
            (func (export "snapshot") (result i64) {snapshot})
            (func (export "restore") (param i32 i32) (result i64) {restore}))"#
        ),
    )
    .unwrap();
    let mut guest = Guest::instantiate("wat", &module, "wat").unwrap();
    if let Some(frame) = tick {
        guest
            .exports
            .memory
            .write(&mut guest.store, top - frame.len(), frame)
            .unwrap();
    }
    guest
}

/// `fresh` prepared as the replacement of `old`, seated as a drawn view.
fn swap(mut old: Guest, fresh: Guest) -> (Result<Guest, Failure>, Arc<Mutex<Mounted>>) {
    old.ticks = 1;
    let alive = old.alive.clone();
    let mounted = Mounted::seat();
    mounted.lock().unwrap().slot = Slot::Ready(Box::new(old));
    let mut ticks = 0;
    let mut timing = LoadTiming::default();
    let swapped = Guest::replacement(fresh, &alive, &mut ticks, &mounted, "wat", &mut timing);
    (swapped, mounted)
}

fn init_ran(guest: &Guest) -> bool {
    guest.exports.memory.data(&guest.store)[2] == 1
}

/// Every arm of `Guest::replacement` a real view cannot be made to take on
/// demand: the state past its budget, the guest refusing the state and
/// starting clean, the first frame trapping, and the drawn view's own
/// snapshot trapping.
#[test]
fn a_replacement_takes_the_state_it_is_handed_or_says_why_not() {
    let frame = wire::encode(&wire::Frame {
        root: Some(wire::Node::empty()),
        ..Default::default()
    });
    let ok = |len: usize| format!("i64.const {}", wire::abi::pack(0, len as u32));
    let refused = format!("i64.const {}", wire::abi::pack(1, 1));

    // one byte past the budget (the tag not counted) is refused before a
    // byte of it is copied; the budget itself carries over whole
    let (swapped, _) = swap(
        wat_view(&ok(wire::MAX_SNAPSHOT_BYTES + 2), &ok(1), Some(&frame)),
        wat_view(&ok(1), &ok(1), Some(&frame)),
    );
    assert!(matches!(
        swapped.err(),
        Some(Failure::Refused(why)) if why.contains("snapshot byte budget")
    ));
    let (swapped, _) = swap(
        wat_view(&ok(wire::MAX_SNAPSHOT_BYTES + 1), &ok(1), Some(&frame)),
        wat_view(&ok(1), &ok(1), Some(&frame)),
    );
    let fresh = swapped.expect("a full budget carries over");
    assert!(fresh.staged && !init_ran(&fresh), "restored, not inited");

    // the guest refuses the state as not its own: it inits instead
    let (swapped, _) = swap(
        wat_view(&ok(1), &ok(1), Some(&frame)),
        wat_view(&ok(1), &refused, Some(&frame)),
    );
    let fresh = swapped.expect("a refused state starts clean");
    assert!(
        fresh.staged && init_ran(&fresh),
        "inited in the state's place"
    );

    // the first frame traps: no replacement
    let (swapped, _) = swap(
        wat_view(&ok(1), &ok(1), Some(&frame)),
        wat_view(&ok(1), &ok(1), None),
    );
    assert!(matches!(swapped.err(), Some(Failure::Trapped(_))));

    // the drawn view's snapshot traps: the load fails, and the view it
    // leaves seated is faulted rather than entered again
    let (swapped, mounted) = swap(
        wat_view("unreachable", &ok(1), Some(&frame)),
        wat_view(&ok(1), &ok(1), Some(&frame)),
    );
    assert!(matches!(swapped.err(), Some(Failure::Trapped(_))));
    assert!(matches!(
        &mounted.lock().unwrap().slot,
        Slot::Ready(old) if old.fault.is_some()
    ));
}
