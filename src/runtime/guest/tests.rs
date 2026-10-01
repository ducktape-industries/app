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

/// A response for a request no route in the frame names (here 8, the frame
/// holding 7) is merged into nothing.
#[test]
fn a_tooltip_response_for_a_request_not_in_the_frame_attaches_nowhere() {
    let mut held = Some(tooltip_route(7));
    let mut frame = wire::Frame {
        unchanged: true,
        tooltip_responses: vec![wire::TooltipResponse {
            request: 8,
            character_index: None,
            content: Some(Box::new(wire::Node::empty())),
        }],
        ..Default::default()
    };
    assert!(!merge(&mut held, &mut frame).unwrap().0);
    assert!(
        primitive_tooltip(frame.root.as_ref().unwrap())
            .content
            .is_none()
    );
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

/// Two authored routes, of different kinds, naming one request: refused,
/// and neither is written.
#[test]
fn tooltip_response_rejects_duplicate_authored_routes_before_mutating() {
    let mut held = Some(wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: Default::default(),
        interactivity: Default::default(),
        children: vec![tooltip_route(7), primitive_tooltip_route("svg", 7)],
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

/// The held tree passed sanitize when it arrived: an `unchanged` frame with
/// no tooltip response takes it as it is, without another walk. Seen
/// through a held tree no frame could have brought (twice the node cap),
/// which a walk would cut.
#[test]
fn an_unchanged_frame_takes_the_held_tree_without_walking_it() {
    let oversized = wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: Default::default(),
        interactivity: Default::default(),
        children: (0..2 * wire::MAX_NODES)
            .map(|_| wire::Node::empty())
            .collect(),
    });
    let mut held = Some(oversized.clone());
    let mut frame = wire::Frame {
        unchanged: true,
        ..Default::default()
    };
    assert_eq!(
        merge(&mut held, &mut frame),
        Ok((false, Default::default()))
    );
    assert_eq!(frame.root, Some(oversized), "the held tree, unwalked");
}

/// A view in WAT whose every tick answers the frame [`draw`] last wrote
/// into its memory: the test plays the guest one frame at a time.
fn scripted_view() -> Guest {
    let module = wasmtime::Module::new(
        engine(),
        r#"(module
            (memory (export "memory") 40)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) (i64.load (i32.const 8)))
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#,
    )
    .unwrap();
    Guest::instantiate("scripted", &module, "scripted").unwrap()
}

/// One tick of `guest`, answering with a frame whose tree is `children`.
fn draw(guest: &mut Guest, children: Vec<wire::Node>) {
    let frame = wire::encode(&wire::Frame {
        root: Some(wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: Default::default(),
            interactivity: Default::default(),
            children,
        })),
        ..Default::default()
    });
    let memory = guest.exports.memory;
    let at = memory.data_size(&guest.store) - frame.len();
    memory.write(&mut guest.store, at, &frame).unwrap();
    let answer = wire::abi::pack(at as u32, frame.len() as u32);
    memory
        .write(&mut guest.store, 8, &answer.to_le_bytes())
        .unwrap();
    guest.tick();
    assert_eq!(guest.fault, None);
}

/// An Image under `hash`, bringing `bytes` of picture or naming it alone.
fn image(hash: u64, bytes: Option<usize>) -> wire::Node {
    wire::Node::Image {
        id: None,
        hash,
        data: bytes.map(|len| wire::ImageData::Encoded(vec![hash as u8; len])),
        label: None,
        image_style: wire::ImageStyle {
            grayscale: false,
            object_fit: wire::ImageObjectFit::Contain,
        },
        loading: false,
        fallback: false,
        state_children: Vec::new(),
        style: Default::default(),
        interactivity: Default::default(),
    }
}

/// The picture bytes a tree carries in its own nodes.
fn inline_picture_bytes(mut root: wire::Node) -> usize {
    let mut bytes = 0;
    root.for_each_mut(&mut |node| match node {
        wire::Node::Image {
            data: Some(data), ..
        } => bytes += data.byte_len(),
        wire::Node::Svg {
            source: wire::SvgSource::Data {
                bytes: Some(data), ..
            },
            ..
        } => bytes += data.len(),
        _ => {}
    });
    bytes
}

/// One 1 MiB picture named by 8,000 nodes is held once: the tree the seat
/// hands its renderer names it by hash, and the bytes behind the hash are
/// the store's one copy, not one per node (8 GiB).
#[test]
fn a_picture_named_by_every_node_is_held_once() {
    const PICTURE: usize = wire::MAX_PICTURE_BYTES_PER_FRAME;
    let mut guest = scripted_view();
    draw(&mut guest, vec![image(1, Some(PICTURE))]);
    draw(&mut guest, (0..8_000).map(|_| image(1, None)).collect());
    let (root, held) = guest.drawn();
    assert_eq!(root.count(), 8_001);
    assert_eq!(
        inline_picture_bytes(root),
        0,
        "the tree drawn names the picture by hash alone"
    );
    assert_eq!(held.raster.len(), 1);
    assert_eq!(guest.pictures.bytes(), PICTURE as u64);
    assert!(
        Arc::ptr_eq(&held, &guest.pictures.held()),
        "the renderer reads the store's own bytes"
    );
}

/// A view drawing a fresh 1 MiB picture on every tick holds no more than
/// the seat's budget. Past it the pictures its tree no longer names go,
/// and the view is told to resync: it forgets what it sent, so a picture
/// it draws again comes with its bytes and is held again.
#[test]
fn a_view_drawing_a_fresh_picture_every_tick_stays_in_its_budget() {
    use crate::runtime::pictures::MAX_PICTURE_BYTES;
    const PICTURE: usize = wire::MAX_PICTURE_BYTES_PER_FRAME;
    let mut guest = scripted_view();
    let mut told = None;
    for hash in 0..MAX_PICTURE_BYTES / PICTURE as u64 + 2 {
        draw(&mut guest, vec![image(hash, Some(PICTURE))]);
        assert!(
            guest.pictures.bytes() <= MAX_PICTURE_BYTES,
            "{} bytes held after picture {hash}",
            guest.pictures.bytes()
        );
        if told.is_none() && guest.pending.contains(&wire::Event::Resync) {
            told = Some(hash);
        }
    }
    let told = told.expect("past the budget the view is told to resync");
    assert_eq!(
        told,
        MAX_PICTURE_BYTES / PICTURE as u64,
        "the first picture past it"
    );
    let held = guest.pictures.held();
    assert!(held.raster.contains_key(&told), "what the tree names stays");
    assert!(
        !held.raster.contains_key(&0),
        "what it no longer names went"
    );

    draw(&mut guest, vec![image(0, Some(PICTURE))]);
    assert!(
        guest.pictures.held().raster.contains_key(&0),
        "drawn again, held again"
    );
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
    assert_eq!(old.min_width, 480, "the probe declares none: the default");
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
    let Ok(Snapshot::Taken(state)) = old.snapshot() else {
        panic!("no state handed over");
    };
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
    let fresh = Guest::replacement(
        fresh,
        &alive,
        &mut ticks,
        &mounted,
        &code,
        "probe",
        &mut timing,
    )
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

/// A module's minimum width is its drawn or compiled view's: none while the
/// seat loads.
#[test]
fn a_view_says_its_min_width_once_it_is_drawn() {
    let seat = Mounted::seat();
    registry()
        .lock()
        .unwrap()
        .insert(("min-width-seat", 7), seat.clone());
    assert_eq!(min_width("min-width-seat"), None, "loading");
    seat.lock().unwrap().slot = Slot::Compiled {
        name: String::new(),
        min_width: 640,
    };
    assert_eq!(min_width("min-width-seat"), Some(640.), "its manifest's");
    let mut guest = wat_view("unreachable", "unreachable", None);
    guest.min_width = 680;
    seat.lock().unwrap().slot = Slot::Ready(Box::new(guest));
    assert_eq!(min_width("min-width-seat"), Some(680.));
    registry().lock().unwrap().remove(&("min-width-seat", 7));
    assert_eq!(min_width("min-width-seat"), None, "gone");
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

/// The code of a view in WAT whose `snapshot`, `restore` and `tick` are
/// the bodies given, each leaving the packed `i64` it answers with. Memory
/// byte 0 is a result's Ok tag and byte 1 a refusal's; `init` marks byte 2
/// so a test sees whether it ran; `tick`'s frame sits at the top of memory,
/// clear of the buffer `alloc` hands out at 64.
fn wat_code(snapshot: &str, restore: &str, tick: Option<&[u8]>) -> wasmtime::Module {
    let top = SWAP_PAGES * 65536;
    let (tick_body, frame) = match tick {
        Some(frame) => (
            format!(
                "i64.const {}",
                wire::abi::pack((top - frame.len()) as u32, frame.len() as u32)
            ),
            format!(
                r#"(data (i32.const {}) "{}")"#,
                top - frame.len(),
                frame
                    .iter()
                    .map(|byte| format!("\\{byte:02x}"))
                    .collect::<String>()
            ),
        ),
        None => ("unreachable".to_string(), String::new()),
    };
    wasmtime::Module::new(
        engine(),
        format!(
            r#"(module
            (memory (export "memory") {SWAP_PAGES})
            (data (i32.const 1) "\01")
            {frame}
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init") (i32.store8 (i32.const 2) (i32.const 1)))
            (func (export "tick") (param i32 i32) (result i64) {tick_body})
            (func (export "snapshot") (result i64) {snapshot})
            (func (export "restore") (param i32 i32) (result i64) {restore}))"#
        ),
    )
    .unwrap()
}

/// A drawn instance of [`wat_code`].
fn wat_view(snapshot: &str, restore: &str, tick: Option<&[u8]>) -> Guest {
    Guest::instantiate("wat", &wat_code(snapshot, restore, tick), "wat").unwrap()
}

/// An instance of `code` prepared as the replacement of `old`, seated as a
/// drawn view that ticked.
fn swap(mut old: Guest, code: wasmtime::Module) -> (Result<Guest, Failure>, Arc<Mutex<Mounted>>) {
    old.ticks = 1;
    let alive = old.alive.clone();
    let mounted = Mounted::seat();
    mounted.lock().unwrap().slot = Slot::Ready(Box::new(old));
    let fresh = Guest::instantiate("wat", &code, "wat").unwrap();
    let mut ticks = 0;
    let mut timing = LoadTiming::default();
    let swapped = Guest::replacement(
        fresh,
        &alive,
        &mut ticks,
        &mounted,
        &code,
        "wat",
        &mut timing,
    );
    (swapped, mounted)
}

fn init_ran(guest: &Guest) -> bool {
    guest.exports.memory.data(&guest.store)[2] == 1
}

/// Every arm of `Guest::replacement` a real view cannot be made to take on
/// demand: the state past its budget, the guest refusing the state, the
/// restore trapping and the drawn view's own snapshot trapping (each of
/// those starts clean), and the first frame trapping (no replacement).
#[test]
fn a_replacement_takes_the_state_it_is_handed_or_says_why_not() {
    let frame = wire::encode(&wire::Frame {
        root: Some(wire::Node::empty()),
        ..Default::default()
    });
    let ok = |len: usize| format!("i64.const {}", wire::abi::pack(0, len as u32));
    let refused = format!("i64.const {}", wire::abi::pack(1, 1));

    // one byte past the budget (the tag not counted) is refused before a
    // byte of it is copied, and the replacement starts clean; the budget
    // itself carries over whole
    let (swapped, mounted) = swap(
        wat_view(&ok(wire::MAX_SNAPSHOT_BYTES + 2), &ok(1), Some(&frame)),
        wat_code(&ok(1), &ok(1), Some(&frame)),
    );
    let fresh = swapped.expect("a state past the budget starts clean");
    assert!(fresh.staged && init_ran(&fresh), "inited, not restored");
    assert!(matches!(
        &mounted.lock().unwrap().slot,
        Slot::Ready(old) if old.fault.is_none()
    ));
    let (swapped, _) = swap(
        wat_view(&ok(wire::MAX_SNAPSHOT_BYTES + 1), &ok(1), Some(&frame)),
        wat_code(&ok(1), &ok(1), Some(&frame)),
    );
    let fresh = swapped.expect("a full budget carries over");
    assert!(fresh.staged && !init_ran(&fresh), "restored, not inited");

    // the guest refuses the state as not its own: it inits instead
    let (swapped, _) = swap(
        wat_view(&ok(1), &ok(1), Some(&frame)),
        wat_code(&ok(1), &refused, Some(&frame)),
    );
    let fresh = swapped.expect("a refused state starts clean");
    assert!(
        fresh.staged && init_ran(&fresh),
        "inited in the state's place"
    );

    // the restore traps, its memory half-written (byte 3): a new instance,
    // never entered before, inits
    let (swapped, _) = swap(
        wat_view(&ok(1), &ok(1), Some(&frame)),
        wat_code(
            &ok(1),
            "(i32.store8 (i32.const 3) (i32.const 1)) unreachable",
            Some(&frame),
        ),
    );
    let fresh = swapped.expect("a restore that traps starts clean");
    assert!(fresh.staged && fresh.fault.is_none() && init_ran(&fresh));
    assert_eq!(
        fresh.exports.memory.data(&fresh.store)[3],
        0,
        "not the instance the trap left half-written"
    );

    // the first frame traps: no replacement
    let (swapped, _) = swap(
        wat_view(&ok(1), &ok(1), Some(&frame)),
        wat_code(&ok(1), &ok(1), None),
    );
    assert!(matches!(swapped.err(), Some(Failure::Trapped(_))));

    // the drawn view's snapshot traps: the view it leaves seated is
    // faulted rather than entered again, and the replacement starts clean
    let (swapped, mounted) = swap(
        wat_view("unreachable", &ok(1), Some(&frame)),
        wat_code(&ok(1), &ok(1), Some(&frame)),
    );
    let fresh = swapped.expect("a snapshot that traps starts clean");
    assert!(fresh.staged && init_ran(&fresh));
    assert!(matches!(
        &mounted.lock().unwrap().slot,
        Slot::Ready(old) if old.fault.is_some()
    ));
}

/// A view that stopped is replaced by the next deployment, which starts
/// clean: the stopped instance is not entered for a snapshot. Before, a
/// trap held every replacement off ("pending work") until a manual Retry.
#[test]
fn a_stopped_view_is_replaced_by_the_next_deployment() {
    let frame = wire::encode(&wire::Frame {
        root: Some(wire::Node::empty()),
        ..Default::default()
    });
    let ok = |len: usize| format!("i64.const {}", wire::abi::pack(0, len as u32));
    let mut old = wat_view(&ok(1), &ok(1), Some(&frame));
    old.fault = Some("trapped for the test".into());
    let (swapped, _) = swap(old, wat_code(&ok(1), &ok(1), Some(&frame)));
    let fresh = swapped.expect("a stopped view is replaced");
    assert!(fresh.staged && init_ran(&fresh), "started clean");
}

/// The host leaves busy (out of budget, ticking again soon) to the guest:
/// a view whose snapshot answers mid-work swaps with its state, and one
/// that says it cannot hand it over yet holds its replacement off, its
/// state kept, in its own words.
#[test]
fn a_busy_view_swaps_when_its_snapshot_answers_and_waits_when_it_does_not() {
    let frame = wire::encode(&wire::Frame {
        root: Some(wire::Node::empty()),
        ..Default::default()
    });
    let ok = |len: usize| format!("i64.const {}", wire::abi::pack(0, len as u32));
    let refused = format!("i64.const {}", wire::abi::pack(1, 1));
    let mut old = wat_view(&ok(1), &ok(1), Some(&frame));
    old.frame.busy = true;
    let (swapped, _) = swap(old, wat_code(&ok(1), &ok(1), Some(&frame)));
    let fresh = swapped.expect("a busy view whose snapshot answers is swapped");
    assert!(fresh.staged && !init_ran(&fresh), "restored, not inited");

    let mut old = wat_view(&refused, &ok(1), Some(&frame));
    old.frame.busy = true;
    let (swapped, mounted) = swap(old, wat_code(&ok(1), &ok(1), Some(&frame)));
    assert!(matches!(
        swapped.err(),
        Some(Failure::Refused(why)) if why.contains("does not hand its state over yet")
    ));
    assert!(matches!(
        &mounted.lock().unwrap().slot,
        Slot::Ready(old) if old.fault.is_none()
    ));
}

/// A view's code in WAT that counts its ticks in memory byte 16 and hands
/// the count over as its state: `init` starts it at 100, `restore` at the
/// count handed over. Its first tick asks for `clock.ticks` every 16 ms;
/// every tick draws an empty tree.
fn counting_code() -> Module {
    let frame = |requests| wire::Frame {
        root: Some(wire::Node::empty()),
        requests,
        ..Default::default()
    };
    let clock = wire::Request {
        id: 1,
        kind: "clock.ticks".into(),
        payload: wire::methods::encode(&16i64),
    };
    let (first, first_len) = wat_frame(&frame(vec![clock]));
    let (then, then_len) = wat_frame(&frame(Vec::new()));
    let first_tick = wire::abi::pack(65536, first_len);
    let then_tick = wire::abi::pack(69632, then_len);
    let state = wire::abi::pack(32, 2);
    let restored = wire::abi::pack(48, 1);
    Module::new(
        guest::engine(),
        format!(
            r#"(module
            (memory (export "memory") 2)
            (global $n (mut i32) (i32.const 0))
            (data (i32.const 65536) "{first}")
            (data (i32.const 69632) "{then}")
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init") (i32.store8 (i32.const 16) (i32.const 100)))
            (func (export "tick") (param i32 i32) (result i64)
                (i32.store8 (i32.const 16) (i32.add (i32.load8_u (i32.const 16)) (i32.const 1)))
                global.get $n i32.const 1 i32.add global.set $n
                global.get $n i32.const 1 i32.le_u
                if (result i64) i64.const {first_tick} else i64.const {then_tick} end)
            (func (export "snapshot") (result i64)
                (i32.store8 (i32.const 33) (i32.load8_u (i32.const 16)))
                i64.const {state})
            (func (export "restore") (param i32 i32) (result i64)
                (i32.store8 (i32.const 16) (i32.load8_u (local.get 0)))
                i64.const {restored}))"#
        ),
    )
    .unwrap()
}

/// The count a [`counting_code`] view holds.
fn count(guest: &Guest) -> u8 {
    guest.exports.memory.data(&guest.store)[16]
}

/// A seat holding a [`counting_code`] view of `module`, inited and drawn
/// once, so it ticks on its clock; the code, the view's `alive`, and the
/// load a block would start for a new deployment of it.
fn counting_seat(module: &'static str) -> (Arc<Module>, Arc<()>, Load) {
    let code = Arc::new(counting_code());
    let mut old = Guest::instantiate(module, &code, module).unwrap();
    old.capabilities = vec![Capability::Clock];
    old.init(module).unwrap();
    old.redraw(&None);
    assert_eq!((old.ticks, count(&old), old.clocks.len()), (1, 101, 1));
    let alive = old.alive.clone();
    let seat = Mounted::seat();
    lock(&seat).slot = Slot::Ready(Box::new(old));
    let generation = lock(&seat).start();
    let load = Load {
        module,
        seat,
        generation,
        asked_of: connection().lock().unwrap().clone(),
        code: Some((::abi::BlobId::Sha256([7; 32]), false)),
    };
    (code, alive, load)
}

/// The window thread's turn of the drawn view in `seat`, `after` a pause:
/// what its clock, or an event, makes it do.
fn turn_drawn(seat: &Mutex<Mounted>, after: Duration) -> u64 {
    std::thread::sleep(after);
    let mut locked = lock(seat);
    let Slot::Ready(old) = &mut locked.slot else {
        panic!("no view drawn");
    };
    old.redraw(&None);
    old.ticks
}

/// A new deployment lands on a view that ticks on a clock, WITH its state,
/// though the clock ticked the drawn view while its replacement was being
/// prepared. Before, the install refused it ("the view moved…") and no
/// later block asked again: the person kept the old code for good.
#[test]
fn a_view_ticking_on_a_clock_swaps_to_new_code_with_its_state() {
    let (code, alive, load) = counting_seat("ticking-swap");
    let seat = load.seat.clone();
    load.land(|load, timing| {
        let mut fresh = Guest::instantiate(load.module, &code, load.module).unwrap();
        // as `load` seats the manifest's targets on what it prepares
        fresh.targets = vec!["chat".into()];
        let mut ticks = 0;
        let fresh = Guest::replacement(
            fresh,
            &alive,
            &mut ticks,
            &load.seat,
            &code,
            load.module,
            timing,
        )
        .expect("prepared");
        assert_eq!(count(&fresh), 102, "the state at the handover, ticked once");
        // the drawn view's clock fires while the replacement is on its way
        assert_eq!(turn_drawn(&load.seat, Duration::from_millis(20)), ticks + 1);
        Ok(Loaded::Swap {
            fresh: Box::new(fresh),
            alive: alive.clone(),
            ticks,
            code: code.clone(),
            shown: load.module.into(),
        })
    });
    let locked = lock(&seat);
    let Slot::Ready(seated) = &locked.slot else {
        panic!("no view seated");
    };
    assert!(
        !Arc::ptr_eq(&seated.alive, &alive),
        "the new code took the seat"
    );
    assert_eq!(
        count(seated),
        103,
        "with the state the drawn view held at the swap, ticked once"
    );
    assert_eq!(
        seated.targets,
        ["chat"],
        "the manifest's targets are seated, so its node methods still reach the program"
    );
    assert!(locked.retry.is_none());
}

/// A swap the install refuses (here: an event waits for the drawn view's
/// next turn, so it is not settled) is asked again by a later block.
/// Before, the refusal cleared the hold-off and no block asked again.
#[test]
fn a_swap_refused_at_install_is_asked_again() {
    let (code, alive, load) = counting_seat("refused-swap");
    let seat = load.seat.clone();
    // prepared, and then, if `event`, an event comes in for the drawn view
    let (code, alive) = (&code, &alive);
    let prepared = |event: bool| {
        move |load: &Load, timing: &mut LoadTiming| {
            let fresh = Guest::instantiate(load.module, code, load.module).unwrap();
            let mut ticks = 0;
            let fresh = Guest::replacement(
                fresh,
                alive,
                &mut ticks,
                &load.seat,
                code,
                load.module,
                timing,
            )
            .expect("prepared");
            let Slot::Ready(old) = &mut lock(&load.seat).slot else {
                panic!("no view drawn");
            };
            if event {
                old.pending.push(wire::Event::Resync);
            }
            Ok(Loaded::Swap {
                fresh: Box::new(fresh),
                alive: alive.clone(),
                ticks,
                code: code.clone(),
                shown: load.module.into(),
            })
        }
    };
    load.land(prepared(true));
    {
        let locked = lock(&seat);
        assert!(matches!(&locked.slot, Slot::Ready(old) if Arc::ptr_eq(&old.alive, alive)));
        let code = code_digest(&::abi::BlobId::Sha256([7; 32]));
        let retry = locked.retry.as_ref().expect("the refused swap is held off");
        assert_eq!(retry.code, Some(code), "under the code the roster names");
        let due = Instant::now() + RETRY_FIRST * 2;
        assert!(
            locked.reload_due(true, true, code, due),
            "a later block asks again"
        );
    }
    // the next block's load, once the drawn view's turn has taken the
    // event: it swaps, with the state the view holds then
    turn_drawn(&seat, Duration::ZERO);
    let generation = lock(&seat).start();
    let asked_of = connection().lock().unwrap().clone();
    Load {
        module: "refused-swap",
        seat: seat.clone(),
        generation,
        asked_of,
        code: Some((::abi::BlobId::Sha256([7; 32]), false)),
    }
    .land(prepared(false));
    let locked = lock(&seat);
    let Slot::Ready(seated) = &locked.slot else {
        panic!("no view seated");
    };
    assert!(!Arc::ptr_eq(&seated.alive, alive), "asked again, it landed");
    assert_eq!(count(seated), 103, "with the state the view held then");
    assert!(locked.retry.is_none());
}

/// The hold a swap takes of its seat's lock when the drawn view moved while
/// its replacement was prepared: the handover taken again into a new
/// instance, restored and its first tree verified, the window thread
/// waiting meanwhile. The view is the one `DUCKTAPE_VIEW_PROBE` names
/// (chat_view.wasm, say), inited and drawn once. Run it alone:
/// `cargo test the_hold -- --ignored --exact --nocapture`.
#[test]
#[ignore = "measurement: needs DUCKTAPE_VIEW_PROBE=<a view's wasm>, prints the hold"]
fn the_hold_of_a_swap_handed_over_again() {
    let path = std::env::var("DUCKTAPE_VIEW_PROBE").expect("DUCKTAPE_VIEW_PROBE");
    let bytes = std::fs::read(path).expect("probe bytes");
    let code = Guest::compile(&bytes, "probe")
        .map_err(|f| f.to_string())
        .expect("compiles");
    let mut old = Guest::from_bytes("probe", &bytes, "probe").expect("loads");
    // turned as the window thread turns it until it hands its state over:
    // its first requests answered (refused, with no node) and taken in
    let settled_by = Instant::now() + Duration::from_secs(5);
    loop {
        old.redraw(&None);
        match old.snapshot() {
            Ok(Snapshot::Taken(_)) if old.settled() => break,
            other => assert!(
                Instant::now() < settled_by,
                "the view never settled: {:?}",
                other.map(|answer| match answer {
                    Snapshot::Refused(refusal) => refusal,
                    _ => "past the budget".into(),
                })
            ),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let like = Guest::instantiate("probe", &code, "probe").expect("instantiates");
    let mut held: Vec<_> = (0..20)
        .map(|_| {
            let mut timing = LoadTiming::default();
            let taken = Instant::now();
            let fresh = Guest::prepared_again(&mut old, &like, &code, "probe", &mut timing)
                .expect("handed over again");
            let held = taken.elapsed();
            assert!(fresh.staged && fresh.fault.is_none());
            (held, timing.snapshot_bytes)
        })
        .collect();
    held.sort();
    let ms = |at: usize| held[at].0.as_secs_f64() * 1000.;
    println!(
        "hold over 20 swaps, {} B of state: min {:.2} ms, median {:.2} ms, max {:.2} ms",
        held[0].1,
        ms(0),
        ms(10),
        ms(19)
    );
}

/// The budget's cost, measured: the process's resident memory around a
/// view that fills its seat's picture budget (64 pictures, every one named,
/// the first by 8,000 more nodes), hands the tree to be drawn, then keeps
/// drawing fresh pictures past the budget. `picture` is each picture's
/// size: the control run draws the same trees with 1-byte pictures. Run
/// each alone: `cargo test <name> -- --ignored --exact --nocapture`.
fn resident_at_the_picture_budget(picture: usize) {
    use crate::runtime::pictures::MAX_PICTURE_BYTES;
    fn resident_mib() -> f64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
        let kib: f64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        kib / 1024.
    }
    let full = MAX_PICTURE_BYTES / wire::MAX_PICTURE_BYTES_PER_FRAME as u64;
    let tree = |last: u64| {
        let mut children: Vec<_> = (0..last).map(|hash| image(hash, None)).collect();
        children.push(image(last, Some(picture)));
        children.extend((0..8_000).map(|_| image(0, None)));
        children
    };
    let mut guest = scripted_view();
    draw(&mut guest, Vec::new());
    let before = resident_mib();
    for last in 0..full {
        draw(&mut guest, tree(last));
    }
    let filled = resident_mib();
    let drawn = guest.drawn();
    let handed = resident_mib();
    let (nodes, held) = (
        drawn.0.count(),
        drawn
            .1
            .raster
            .values()
            .map(|data| data.byte_len())
            .sum::<usize>(),
    );
    // the seat hands its tree the store's bytes anew with every frame
    drop(drawn);
    for last in full..2 * full {
        draw(&mut guest, vec![image(last, Some(picture))]);
    }
    let churned = resident_mib();
    println!(
        "pictures of {picture} B, held {held} B: resident +{:.1} MiB filled, +{:.1} MiB with the tree handed over ({nodes} nodes), +{:.1} MiB after {full} more fresh pictures (held then {} B)",
        filled - before,
        handed - before,
        churned - before,
        guest.pictures.bytes(),
    );
}

#[test]
#[ignore = "measurement: prints resident memory, asserts nothing of it"]
fn a_seat_at_its_picture_budget() {
    resident_at_the_picture_budget(wire::MAX_PICTURE_BYTES_PER_FRAME);
}

#[test]
#[ignore = "measurement: the control for a_seat_at_its_picture_budget"]
fn a_seat_drawing_the_same_trees_with_one_byte_pictures() {
    resident_at_the_picture_budget(1);
}
