use super::*;

fn label(view: &View) -> (String, u32) {
    match view.frame.root.as_ref().expect("a tree") {
        wire::Node::Container(wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name(id)),
            interactivity: Some(interactivity),
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

/// The held tree passed sanitize when it arrived: an `unchanged` frame
/// takes it as it is, without another walk. Seen through a held tree no
/// frame could have brought (nested past the depth bound), which a walk
/// would cut.
#[test]
fn an_unchanged_frame_takes_the_held_tree_without_walking_it() {
    let oversized = (0..wire::MAX_DEPTH + 2).fold(wire::Node::empty(), |child, _| {
        wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: crate::render::plain_style(),
            interactivity: Default::default(),
            children: vec![child],
        })
    });
    let mut held = Some(oversized.clone());
    let mut frame = wire::Frame {
        unchanged: true,
        ..Default::default()
    };
    assert_eq!(
        merge(&mut held, &mut frame, &wire::Styles::default()),
        Ok((false, Default::default()))
    );
    assert_eq!(frame.root, Some(oversized), "the held tree, unwalked");
}

/// A view in WAT whose every tick answers the frame [`draw`] last wrote
/// into its memory: the test plays the view one frame at a time.
fn scripted_view() -> View {
    let module = crate::runtime::view::prepared_for_test(
        r#"(module
            (memory (export "memory") 40)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) (i64.load (i32.const 8)))
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#,
    );
    View::instantiate("scripted", crate::contract::runner(), module, "scripted").unwrap()
}

/// One tick of `view`, answering with a frame whose tree is `children`.
fn draw(view: &mut View, children: Vec<wire::Node>) {
    answer(
        view,
        &crate::render::sent(wire::Frame {
            root: Some(wire::Node::Container(view_wire::ContainerNode {
                id: None,
                style: crate::render::plain_style(),
                interactivity: Default::default(),
                children,
            })),
            ..Default::default()
        }),
    );
    assert_eq!(view.fault, None);
}

/// One tick of `view`, answering with `frame` as it stands.
fn answer(view: &mut View, frame: &wire::Frame) {
    answer_bytes(view, &wire::encode(frame));
}

/// One tick of `view`, answering with `frame`'s bytes as they stand.
fn answer_bytes(view: &mut View, frame: &[u8]) {
    let memory = view.memory_mut_for_test();
    let at = memory.len() - frame.len();
    memory[at..].copy_from_slice(frame);
    let answer = wire::abi::pack(at as u32, frame.len() as u32);
    memory[8..16].copy_from_slice(&answer.to_le_bytes());
    view.tick();
}

/// The host holds the style table of the tree it holds: a whole frame's
/// entries are the table, a patch frame's join it, and the next whole
/// frame's replace it. A node naming a style the table does not hold ends
/// the view; the host never draws it with a style of its own choosing.
#[test]
fn the_host_holds_a_trees_styles_and_refuses_a_style_it_does_not_hold() {
    use gpui_kit::Styled as _;
    let boxed = |style: u32| {
        wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: wire::StyleId(style),
            interactivity: Default::default(),
            children: Vec::new(),
        })
    };
    let wide = wire::Style::new(gpui_kit::div().w_full().style());
    let tall = wire::Style::new(gpui_kit::div().h_full().style());
    let whole = |style: u32| wire::Frame {
        root: Some(boxed(style)),
        styles: vec![wide.clone()],
        ..Default::default()
    };
    let mut view = scripted_view();
    answer(&mut view, &whole(0));
    assert_eq!((view.fault.as_deref(), view.styles.len()), (None, 1));

    let patch = wire::Frame {
        patches: vec![wire::Patch::Replace {
            path: Vec::new(),
            node: boxed(1),
        }],
        styles: vec![tall],
        ..Default::default()
    };
    answer(&mut view, &patch);
    assert_eq!((view.fault.as_deref(), view.styles.len()), (None, 2));
    assert_eq!(view.frame.root, Some(boxed(1)));
    assert_eq!(
        view.styles[wire::StyleId(1)].size.height,
        gpui_kit::div().h_full().style().size.height,
        "the patch's node names the entry the patch brought"
    );

    answer(&mut view, &whole(0));
    assert_eq!(
        (view.fault.as_deref(), view.styles.len()),
        (None, 1),
        "a whole frame's table is the table"
    );

    // the id the patch's entry had, in a table that no longer holds it
    answer(&mut view, &whole(1));
    assert_eq!(
        view.fault.as_deref(),
        Some("Container at []: a node names a style its table does not hold")
    );
    assert_eq!(
        view.frame.root,
        Some(boxed(0)),
        "the refused tree is not held"
    );
}

/// The Sheet `id` holding `child`, opened at `detents`.
fn sheet(id: &str, detents: Vec<f32>, child: wire::Node) -> wire::Node {
    wire::Node::Sheet {
        id: wire::ElementIdWire::Name(id.into()),
        label: Some("Actions".into()),
        style: crate::render::plain_style(),
        detents,
        on_dismiss: Some(1),
        child: Box::new(child),
    }
}

/// A container whose `on_back` is `handler`.
fn backed(handler: u32, children: Vec<wire::Node>) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: crate::render::plain_style(),
        interactivity: Some(Box::new(wire::Interactivity {
            on_back: Some(handler),
            ..Default::default()
        })),
        children,
    })
}

/// The phone wire's shapes, as a view may break them: one sheet layer per
/// frame, its detents fractions of the body ascending, `on_back` on the
/// root alone. A whole tree that breaks one is refused and ends the view; a
/// patch that would make one is refused, the tree shown stays, and the view
/// is asked for a whole tree.
#[test]
fn a_hostile_sheet_or_back_is_refused() {
    let empty = || wire::Node::empty();
    let refused = |root: wire::Node| {
        let mut view = scripted_view();
        answer(
            &mut view,
            &crate::render::sent(wire::Frame {
                root: Some(root),
                ..Default::default()
            }),
        );
        view.fault.clone()
    };
    let holds = |children| backed(2, vec![sheet("s1", vec![0.6, 1.], children)]);
    assert_eq!(
        refused(holds(empty())),
        None,
        "a root with Back and one sheet"
    );
    for (root, why) in [
        (
            backed(
                2,
                vec![sheet("s2", vec![0.6], sheet("s3", vec![1.], empty()))],
            ),
            "a sheet inside a sheet",
        ),
        (
            backed(
                2,
                vec![
                    sheet("s4", vec![0.6], empty()),
                    sheet("s5", vec![1.], empty()),
                ],
            ),
            "a second sheet in one frame",
        ),
        (
            backed(2, vec![sheet("s6", vec![1., 0.6], empty())]),
            "sheet detents ascend",
        ),
        (
            backed(2, vec![sheet("s7", vec![0.], empty())]),
            "a sheet detent is a fraction of the body",
        ),
        (
            backed(2, vec![sheet("s8", vec![1.5], empty())]),
            "a sheet detent is a fraction of the body",
        ),
        (
            backed(2, vec![sheet("s0", vec![0.5; 9], empty())]),
            "a sheet has at most eight detents",
        ),
        (
            backed(2, vec![backed(3, Vec::new())]),
            "on_back below the root",
        ),
    ] {
        let fault = refused(root).unwrap_or_else(|| panic!("{why}: taken"));
        assert!(fault.contains(why), "{why}: {fault}");
    }

    // the same, brought as a patch onto a tree the host already holds
    let mut view = scripted_view();
    draw(&mut view, vec![sheet("s9", vec![0.6], empty())]);
    let shown = view.frame.root.clone();
    view.pending.clear();
    answer(
        &mut view,
        &crate::render::sent(wire::Frame {
            patches: vec![wire::Patch::Insert {
                path: Vec::new(),
                index: 1,
                node: sheet("s10", vec![1.], empty()),
            }],
            ..Default::default()
        }),
    );
    assert_eq!(view.fault, None);
    assert_eq!(
        view.frame.root, shown,
        "a second sheet by a patch is refused"
    );
    assert_eq!(view.pending, [wire::Event::Resync]);
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
        style: crate::render::plain_style(),
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
    const PICTURE: usize = 1 << 20;
    let mut view = scripted_view();
    draw(&mut view, vec![image(1, Some(PICTURE))]);
    draw(&mut view, (0..8_000).map(|_| image(1, None)).collect());
    let (tree, held) = view.drawn();
    assert_eq!(tree.root.count(), 8_001);
    assert_eq!(
        inline_picture_bytes(tree.root),
        0,
        "the tree drawn names the picture by hash alone"
    );
    assert_eq!(held.raster.len(), 1);
    assert_eq!(view.pictures.bytes(), PICTURE as u64);
    assert!(
        Arc::ptr_eq(&held, &view.pictures.held()),
        "the renderer reads the store's own bytes"
    );
}

/// A view drawing a fresh 1 MiB picture on every tick holds no more than
/// the seat's budget. Past it the pictures its tree no longer names go,
/// and the view is told to resync: it forgets what it sent, so a picture
/// it draws again comes with its bytes and is held again.
#[test]
fn a_view_drawing_a_fresh_picture_every_tick_stays_in_its_budget() {
    use crate::runtime::pictures::charge;
    const PICTURE: usize = 1 << 20;
    let budget = crate::backend::memory_bounds().picture_bytes;
    let fitting = (1..)
        .find(|count: &u64| charge((count + 1) * PICTURE as u64, *count as usize + 1) > budget)
        .unwrap();
    let mut view = scripted_view();
    let mut told = None;
    for hash in 0..fitting + 2 {
        draw(&mut view, vec![image(hash, Some(PICTURE))]);
        assert!(
            view.pictures.bytes() <= budget,
            "{} bytes held after picture {hash}",
            view.pictures.bytes()
        );
        if told.is_none() && view.pending.contains(&wire::Event::Resync) {
            told = Some(hash);
        }
    }
    let told = told.expect("past the budget the view is told to resync");
    assert_eq!(told, fitting, "the first picture past it");
    let held = view.pictures.held();
    assert!(held.raster.contains_key(&told), "what the tree names stays");
    assert!(
        !held.raster.contains_key(&0),
        "what it no longer names went"
    );

    draw(&mut view, vec![image(0, Some(PICTURE))]);
    assert!(
        view.pictures.held().raster.contains_key(&0),
        "drawn again, held again"
    );
}

/// Every export of a real view, through view memory and the swap a
/// deployment takes: the drawn view ticked, its state carried into a fresh
/// instance of the same code, and that instance's first tree drawn from
/// it. The bytes are the view crate's `exported_view` example built for wasm32
/// by the programs repo's `make view-wasm-check`, named by `DUCKTAPE_VIEW_PROBE`.
#[test]
#[ignore = "needs DUCKTAPE_VIEW_PROBE=<exported_view.wasm>"]
fn a_ticked_view_survives_a_swap_with_its_state() {
    let path = std::env::var("DUCKTAPE_VIEW_PROBE").expect("DUCKTAPE_VIEW_PROBE");
    let bytes = std::fs::read(path).expect("probe bytes");
    let mut old = View::from_bytes("probe", &bytes, "probe").expect("loads");
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
    let (code, prepared) = View::prepare(&**crate::contract::runner(), Code::of(&bytes), "probe")
        .map_err(|f| f.to_string())
        .expect("compiles");
    let mut deployment = ViewCode::for_test("", "", 0);
    deployment.hash = code;
    deployment._hold = prepared.hold;
    let mut timing = LoadTiming::default();
    let fresh = futures::executor::block_on(View::replacement(
        "probe",
        0,
        &deployment,
        &alive,
        &mounted,
        "probe",
        &mut timing,
    ))
    .expect("the replacement carries the state");
    assert!(fresh.staged && fresh.fault.is_none());
    assert_eq!(label(&fresh).0, "1", "drawn from the state it was handed");

    // a state that is not the view's own is the view's refusal, not a trap
    let mut next =
        View::instantiate("probe", crate::contract::runner(), code, "probe").expect("instantiates");
    assert!(matches!(
        futures::executor::block_on(next.restore(b"not a snapshot", "probe")),
        Ok(Restored::Refused(_))
    ));
}

/// A program's minimum width is its drawn or compiled view's: none while the
/// seat loads.
#[test]
fn a_view_says_its_min_width_once_it_is_drawn() {
    let seat = Mounted::seat();
    registry()
        .lock()
        .unwrap()
        .insert(("min-width-seat", 7), seat.clone());
    assert_eq!(min_width("min-width-seat"), None, "loading");
    seat.lock().unwrap().slot = Slot::Compiled(ViewCode::for_test("", "", 640));
    assert_eq!(min_width("min-width-seat"), Some(640.), "its manifest's");
    let mut view = wat_view("unreachable", "unreachable", None);
    view.min_width = 680;
    seat.lock().unwrap().slot = Slot::Ready(Box::new(view));
    assert_eq!(min_width("min-width-seat"), Some(680.));
    registry().lock().unwrap().remove(&("min-width-seat", 7));
    assert_eq!(min_width("min-width-seat"), None, "gone");
}

/// A view built against another wire than this app's runs. Where the two
/// read its frame differently it stops, and its seat says what failed, at
/// which node, and both wires; one on this app's wire says what failed.
#[test]
fn a_view_on_another_wire_runs_and_its_failure_names_the_node_and_both_wires() {
    // [`scripted_view`], its manifest naming wire 0
    let bytes = wat::parse_str(format!(
        r#"(module
            (@custom "{}" "ducktape.view.manifest\nForeign\n\n\n320\n0\n\n\nnone")
            (memory (export "memory") 40)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) (i64.load (i32.const 8)))
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#,
        wire::manifest::MANIFEST_SECTION
    ))
    .unwrap();
    let mut view = View::from_bytes("foreign", &bytes, "foreign").expect("loaded all the same");
    assert_eq!((view.name.as_str(), view.wire.as_str()), ("Foreign", "0"));
    // a tree whose second child is a node kind this app does not know
    let space = wire::encode(&wire::Node::Space)[0];
    let mut frame = wire::encode(&crate::render::sent(wire::Frame {
        root: Some(wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: crate::render::plain_style(),
            interactivity: Default::default(),
            children: vec![wire::Node::Space, wire::Node::Space],
        })),
        ..Default::default()
    }));
    let second = frame
        .windows(3)
        .rposition(|bytes| bytes == [0x92, space, space])
        .expect("the two children")
        + 2;
    frame[second] = 0x7f;
    answer_bytes(&mut view, &frame);
    let fault = view.fault.clone().expect("the view stopped");
    assert!(fault.starts_with("node at [1]: "), "{fault}");

    let failure = view.failure(fault.clone());
    assert_eq!(
        failure.title(),
        "This view, built against another wire, stopped"
    );
    assert_eq!(
        failure.to_string(),
        format!(
            "{fault} (this view was built at wire level 0 (0); this app reads level {})",
            wire::WIRE_LEVEL
        )
    );
    view.wire = wire::WIRE_LEVEL.to_string();
    assert_eq!(view.failure(fault.clone()), Failure::Trapped(fault));
}

// The host reads a manifest's line 6 as `Manifest::level` does: a level
// in canonical decimal, else a view built before J-085, at 0.
#[test]
fn a_wire_level_reads_as_the_manifest_reads_it() {
    for line in ["1", "7", "cd5911c62028048f", "01", "0123abcd", "0"] {
        let manifest = wire::manifest::Manifest::parse(&format!(
            "ducktape.view.manifest\nA\n\n\n480\n{line}\n\n\nnone\n"
        ))
        .unwrap();
        assert_eq!(super::level(line), manifest.level(), "{line}");
    }
}

// A node this app could not read is found wherever a frame carries one, by
// what it was, so the view logs each once.
#[test]
fn a_frame_names_the_nodes_this_app_could_not_read() {
    let unknown = |what: &str| wire::Node::Unknown { what: what.into() };
    let frame = wire::Frame {
        root: Some(wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: wire::StyleId(0),
            interactivity: None,
            children: vec![wire::Node::Space, unknown("Node: a")],
        })),
        patches: vec![wire::Patch::Insert {
            path: Vec::new(),
            index: 0,
            node: unknown("Node: b"),
        }],
        ..Default::default()
    };
    assert_eq!(super::unknowns(&frame), ["Node: a", "Node: b"]);
}

/// A large state the swap tests carry over.
const SNAPSHOT_BYTES: usize = 8 << 20;

/// Pages a view's memory holds for the swap tests: the large state plus
/// its tag byte, with room to spare for the events and the frame.
const SWAP_PAGES: usize = SNAPSHOT_BYTES / 65536 + 1;

/// The code of a view in WAT whose `snapshot`, `restore` and `tick` are
/// the bodies given, each leaving the packed `i64` it answers with. Memory
/// byte 0 is a result's Ok tag and byte 1 a refusal's; `init` marks byte 2
/// so a test sees whether it ran; `tick`'s frame sits at the top of memory,
/// clear of the buffer `alloc` hands out at 64.
fn wat_code(snapshot: &str, restore: &str, tick: Option<&[u8]>) -> [u8; 32] {
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
    crate::runtime::view::prepared_for_test(format!(
        r#"(module
            (memory (export "memory") {SWAP_PAGES})
            (data (i32.const 1) "\01")
            {frame}
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init") (i32.store8 (i32.const 2) (i32.const 1)))
            (func (export "tick") (param i32 i32) (result i64) {tick_body})
            (func (export "snapshot") (result i64) {snapshot})
            (func (export "restore") (param i32 i32) (result i64) {restore}))"#
    ))
}

/// A drawn instance of [`wat_code`].
fn wat_view(snapshot: &str, restore: &str, tick: Option<&[u8]>) -> View {
    View::instantiate(
        "wat",
        crate::contract::runner(),
        wat_code(snapshot, restore, tick),
        "wat",
    )
    .unwrap()
}

/// An instance of `code` prepared as the replacement of `old`, seated as a
/// drawn view that ticked.
fn swap(mut old: View, code: [u8; 32]) -> (Result<View, Failure>, Arc<Mutex<Mounted>>) {
    old.ticks = 1;
    let alive = old.alive.clone();
    let mounted = Mounted::seat();
    mounted.lock().unwrap().slot = Slot::Ready(Box::new(old));
    let mut deployment = ViewCode::for_test("", "", 0);
    deployment.hash = code;
    let mut timing = LoadTiming::default();
    let swapped = futures::executor::block_on(View::replacement(
        "wat",
        0,
        &deployment,
        &alive,
        &mounted,
        "wat",
        &mut timing,
    ));
    (swapped, mounted)
}

fn init_ran(view: &View) -> bool {
    view.memory_for_test()[2] == 1
}

/// Every arm of `View::replacement` a real view cannot be made to take on
/// demand: a large state carried whole, the view refusing the state, the
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

    // a large state (the tag not counted) carries over whole
    let (swapped, _) = swap(
        wat_view(&ok(SNAPSHOT_BYTES + 1), &ok(1), Some(&frame)),
        wat_code(&ok(1), &ok(1), Some(&frame)),
    );
    let fresh = swapped.expect("a large state carries over");
    assert!(fresh.staged && !init_ran(&fresh), "restored, not inited");

    // the view refuses the state as not its own: it inits instead
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
        fresh.memory_for_test()[3],
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

/// The host leaves busy (out of budget, ticking again soon) to the view:
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
fn counting_code() -> [u8; 32] {
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
    crate::runtime::view::prepared_for_test(format!(
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
    ))
}

/// The count a [`counting_code`] view holds.
fn count(view: &View) -> u8 {
    view.memory_for_test()[16]
}

/// A seat holding a [`counting_code`] view of `program`, inited and drawn
/// once, so it ticks on its clock; the code, the view's `alive`, and the
/// load a block would start for a new deployment of it.
fn counting_seat(program: &'static str) -> ([u8; 32], Arc<()>, Load) {
    let code = counting_code();
    let mut old = View::instantiate(program, crate::contract::runner(), code, program).unwrap();
    old.capabilities = vec![Capability::Clock];
    old.init(program).unwrap();
    old.redraw(&None);
    assert_eq!((old.ticks, count(&old), old.clocks.len()), (1, 101, 1));
    let alive = old.alive.clone();
    let seat = Mounted::seat();
    lock(&seat).slot = Slot::Ready(Box::new(old));
    let generation = lock(&seat).start();
    let load = Load {
        program,
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

/// The code a new deployment of a [`counting_code`] view prepares.
fn counting_deployment(code: [u8; 32]) -> Box<ViewCode> {
    let mut view = ViewCode::for_test("", "", 320);
    view.hash = code;
    view
}

/// The swap a load parked on `seat`, run as the seat's thread runs it.
fn swap_now(seat: &Arc<Mutex<Mounted>>, program: &'static str) {
    futures::executor::block_on(crate::runtime::seat::swap_in(seat.clone(), program));
}

/// A new deployment lands on a view that ticks on a clock, WITH its state:
/// the load parks the prepared code, the drawn view goes on ticking, and
/// the seat's thread swaps it in with the state it holds then. Before, the
/// install refused a view that ticked meanwhile ("the view moved…") and no
/// later block asked again: the person kept the old code for good.
#[test]
fn a_view_ticking_on_a_clock_swaps_to_new_code_with_its_state() {
    let (code, alive, load) = counting_seat("ticking-swap");
    let seat = load.seat.clone();
    load.land(|load, _| {
        Ok(Loaded::Swap {
            code: counting_deployment(code),
            shown: load.program.into(),
        })
    });
    assert!(lock(&seat).swap.is_some(), "the prepared code is parked");
    // the drawn view's clock fires while the replacement waits
    assert_eq!(turn_drawn(&seat, Duration::from_millis(20)), 2);
    swap_now(&seat, "ticking-swap");
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
    assert!(locked.retry.is_none() && locked.swap.is_none() && !locked.owned);
}

/// A swap the drawn view's handover refuses (here: an event waits for the
/// view's next turn, so it is not settled) keeps the drawn view and is
/// asked again by a later block, which swaps it with the state the view
/// holds then.
#[test]
fn a_swap_refused_at_the_handover_is_asked_again() {
    let (code, alive, load) = counting_seat("refused-swap");
    let seat = load.seat.clone();
    let parked = |load: Load| {
        load.land(|load, _| {
            Ok(Loaded::Swap {
                code: counting_deployment(code),
                shown: load.program.into(),
            })
        })
    };
    parked(load);
    {
        let Slot::Ready(old) = &mut lock(&seat).slot else {
            panic!("no view drawn");
        };
        old.pending.push(wire::Event::Resync);
    }
    swap_now(&seat, "refused-swap");
    {
        let locked = lock(&seat);
        assert!(matches!(&locked.slot, Slot::Ready(old) if Arc::ptr_eq(&old.alive, &alive)));
        let code = code_digest(&::abi::BlobId::Sha256([7; 32]));
        let retry = locked.retry.as_ref().expect("the refused swap is marked");
        assert_eq!(retry.code, None, "no code is held off");
        assert!(
            locked.reload_due(true, true, code),
            "the next roster read asks again"
        );
    }
    // the next block's load, once the drawn view's turn has taken the
    // event: it swaps, with the state the view holds then
    turn_drawn(&seat, Duration::ZERO);
    let generation = lock(&seat).start();
    let asked_of = connection().lock().unwrap().clone();
    parked(Load {
        program: "refused-swap",
        seat: seat.clone(),
        generation,
        asked_of,
        code: Some((::abi::BlobId::Sha256([7; 32]), false)),
    });
    swap_now(&seat, "refused-swap");
    let locked = lock(&seat);
    let Slot::Ready(seated) = &locked.slot else {
        panic!("no view seated");
    };
    assert!(
        !Arc::ptr_eq(&seated.alive, &alive),
        "asked again, it landed"
    );
    assert_eq!(count(seated), 103, "with the state the view held then");
    assert!(locked.retry.is_none());
}

/// The budget's cost, measured: the process's resident memory around a
/// view that fills its seat's picture budget (64 pictures, every one named,
/// the first by 8,000 more nodes), hands the tree to be drawn, then keeps
/// drawing fresh pictures past the budget. `picture` is each picture's
/// size: the control run draws the same trees with 1-byte pictures. Run
/// each alone: `cargo test <name> -- --ignored --exact --nocapture`.
fn resident_at_the_picture_budget(picture: usize) {
    fn resident_mib() -> f64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
        let kib: f64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        kib / 1024.
    }
    let full = crate::backend::memory_bounds().picture_bytes / (1 << 20);
    let tree = |last: u64| {
        let mut children: Vec<_> = (0..last).map(|hash| image(hash, None)).collect();
        children.push(image(last, Some(picture)));
        children.extend((0..8_000).map(|_| image(0, None)));
        children
    };
    let mut view = scripted_view();
    draw(&mut view, Vec::new());
    let before = resident_mib();
    for last in 0..full {
        draw(&mut view, tree(last));
    }
    let filled = resident_mib();
    let drawn = view.drawn();
    let handed = resident_mib();
    let (nodes, held) = (
        drawn.0.root.count(),
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
        draw(&mut view, vec![image(last, Some(picture))]);
    }
    let churned = resident_mib();
    println!(
        "pictures of {picture} B, held {held} B: resident +{:.1} MiB filled, +{:.1} MiB with the tree handed over ({nodes} nodes), +{:.1} MiB after {full} more fresh pictures (held then {} B)",
        filled - before,
        handed - before,
        churned - before,
        view.pictures.bytes(),
    );
}

#[test]
#[ignore = "measurement: prints resident memory, asserts nothing of it"]
fn a_seat_at_its_picture_budget() {
    resident_at_the_picture_budget(1 << 20);
}

#[test]
#[ignore = "measurement: the control for a_seat_at_its_picture_budget"]
fn a_seat_drawing_the_same_trees_with_one_byte_pictures() {
    resident_at_the_picture_budget(1);
}

/// A frame with a web, camera or microphone piece ends a view whose
/// manifest does not declare that piece's capability (another one does
/// not stand in), as a frame the host cannot take does; a view that
/// declares it draws it.
#[test]
fn a_piece_from_a_view_without_its_capability_ends_the_view() {
    let style = crate::render::plain_style();
    let pieces = [
        (
            Capability::Web,
            wire::Node::Web {
                id: wire::ElementIdWire::Name("page".into()),
                url: "https://example.com/".into(),
                style,
                revision: 0,
                commands: Vec::new(),
                on_state: None,
            },
        ),
        (
            Capability::Camera,
            wire::Node::Camera {
                id: wire::ElementIdWire::Name("camera".into()),
                style,
                device: None,
                mirror: true,
                object_fit: wire::ImageObjectFit::Cover,
                on_state: None,
                publish: None,
            },
        ),
        (
            Capability::Microphone,
            wire::Node::Microphone {
                id: wire::ElementIdWire::Name("mic".into()),
                style,
                device: None,
                output: None,
                monitor: false,
                revision: 0,
                commands: Vec::new(),
                on_state: None,
                publish: None,
                muted: false,
            },
        ),
    ];
    for (own, piece) in &pieces {
        let mut declaring = scripted_view();
        declaring.capabilities = vec![*own];
        draw(&mut declaring, vec![piece.clone()]);

        let others: Vec<Capability> = pieces
            .iter()
            .map(|(capability, _)| *capability)
            .filter(|capability| capability != own)
            .collect();
        let mut view = scripted_view();
        view.capabilities = others;
        answer(
            &mut view,
            &crate::render::sent(wire::Frame {
                root: Some(piece.clone()),
                ..Default::default()
            }),
        );
        let name = format!("`{}`", own.as_str());
        assert!(
            view.fault
                .as_deref()
                .is_some_and(|fault| fault.contains(&name)),
            "{name}: {:?}",
            view.fault
        );
    }
}
