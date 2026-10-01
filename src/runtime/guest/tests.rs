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

/// The words a call past `TICK_DEADLINE` ends with.
fn past_the_deadline() -> String {
    format!(
        "the view ran past its {} ms call deadline",
        TICK_DEADLINE.as_millis()
    )
}

/// A tick slow in wall clock but inside its fuel (a pointer chase, a cache
/// miss every four instructions: its fuel lasts ~60M misses, seconds of
/// them) ends at the deadline, in words that say time. A fuel trap and a
/// deadline trap exclude each other, so the words and the time it took are
/// the proof; fuel read back after an epoch trap is stale and proves
/// nothing.
#[test]
fn a_tick_past_the_deadline_is_trapped_by_time_not_fuel() {
    let mut guest = Guest::instantiate("slow", &slow_code_for_test(), "slow").unwrap();
    let started = Instant::now();
    guest.tick();
    let took = started.elapsed();
    assert_eq!(guest.fault, Some(past_the_deadline()));
    assert!(
        took >= TICK_DEADLINE && took < TICK_DEADLINE + Duration::from_secs(3),
        "trapped after {took:?}"
    );
}

/// An honest heavy tick, ~20M fuel of arithmetic, answers its frame.
#[test]
fn an_honest_heavy_tick_is_not_trapped() {
    let (bytes, len) = wat_frame(&wire::Frame {
        root: Some(wire::Node::empty()),
        ..Default::default()
    });
    let tick = wire::abi::pack(65536, len);
    let code = wasmtime::Module::new(
        engine(),
        format!(
            r#"(module
            (memory (export "memory") 2)
            (data (i32.const 65536) "{bytes}")
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) (local $n i32)
                (loop $again
                    (local.set $n (i32.add (local.get $n) (i32.const 1)))
                    (br_if $again (i32.lt_u (local.get $n) (i32.const 2500000))))
                i64.const {tick})
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#
        ),
    )
    .unwrap();
    let mut guest = Guest::instantiate("heavy", &code, "heavy").unwrap();
    guest.tick();
    assert_eq!(guest.fault, None);
    assert!(guest.fuel_used() > 10_000_000, "{} fuel", guest.fuel_used());
    assert!(guest.frame.root.is_some(), "the frame answered");
}

/// The fuel trap keeps its own words: the deadline names only time.
#[test]
fn a_fuel_overrun_is_still_the_fuel_trap() {
    let module = wasmtime::Module::new(
        engine(),
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) (loop (br 0)) i64.const 0)
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#,
    )
    .unwrap();
    let mut guest = Guest::instantiate("spin", &module, "spin").unwrap();
    guest.store.set_fuel(1_000).unwrap();
    // set by hand, not `arm`: a store's own deadline is 0, which traps at
    // the first check, before any fuel could run out
    guest.store.set_epoch_deadline(u64::MAX / 2);
    let error = guest.exports.tick(&mut guest.store, &[]).unwrap_err();
    let words = first_line(&error);
    assert!(
        words.contains("fuel") && !words.contains("deadline"),
        "{words}"
    );
}

/// Every entry into a view is armed with a deadline of its own: a store
/// left with a spent one (deadline 0, a fresh store's own) traps at the
/// first check, so `init`, `snapshot`, `restore` and instantiate (its start
/// function) each run only if they armed it first.
#[test]
fn every_call_into_a_view_sets_its_own_deadline() {
    let ok = format!("i64.const {}", wire::abi::pack(0, 1));
    let mut guest = wat_view(&ok, &ok, None);
    guest.store.set_epoch_deadline(0);
    assert_eq!(guest.init("wat"), Ok(()));
    guest.store.set_epoch_deadline(0);
    assert!(matches!(guest.snapshot(), Ok(Ok(_))));
    guest.store.set_epoch_deadline(0);
    assert!(matches!(
        guest.restore(b"state", "wat"),
        Ok(Restored::Carried)
    ));
    let started = wasmtime::Module::new(
        engine(),
        r#"(module
            (memory (export "memory") 1)
            (func $start)
            (start $start)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) unreachable)
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#,
    )
    .unwrap();
    assert!(Guest::instantiate("start", &started, "start").is_ok());
}

/// The deadline clock runs only while a call does: after a plain tick, a
/// trapped one, a trapped instantiate and a swap's snapshot and restore,
/// no call is left in flight. Other tests arm the same clock at once, so
/// this waits for it to drain rather than reading it once.
#[test]
fn the_deadline_clock_is_released_after_every_call() {
    let frame = wire::encode(&wire::Frame {
        root: Some(wire::Node::empty()),
        ..Default::default()
    });
    let ok = format!("i64.const {}", wire::abi::pack(0, 1));
    let mut plain = wat_view(&ok, &ok, Some(&frame));
    plain.tick();
    let mut slow = Guest::instantiate("slow", &slow_code_for_test(), "slow").unwrap();
    slow.tick();
    assert!(slow.fault.is_some());
    let trapping = wasmtime::Module::new(
        engine(),
        r#"(module
            (memory (export "memory") 1)
            (func $start unreachable)
            (start $start))"#,
    )
    .unwrap();
    assert!(Guest::instantiate("trapping", &trapping, "trapping").is_err());
    let (swapped, _) = swap(plain, wat_view(&ok, &ok, Some(&frame)));
    assert!(swapped.is_ok());
    let drained = Instant::now();
    while IN_FLIGHT.load(Ordering::Acquire) > 0 {
        assert!(
            drained.elapsed() < Duration::from_secs(30),
            "{} calls left in flight",
            IN_FLIGHT.load(Ordering::Acquire)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
