use super::*;

// Decoded pictures belong to presentation state, not one ViewTree entity;
// their bytes are the seat's, handed to every tree it draws.
const PRESENTATION_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><path d="M0 0h1v1H0z"/></svg>"#;

struct Host {
    tree: Entity<ViewTree>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.tree.clone())
    }
}

const RED: [u8; 4] = [255, 0, 0, 255];

fn pictures(with_bytes: bool) -> wire::Node {
    let mut image_style = div().size(px(30.));
    let mut svg_style = div().size(px(30.)).text_color(rgb(0x00ff00));
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("pictures".into())),
        style: crate::render::test_style(
            div()
                .flex()
                .flex_row()
                .size_full()
                .bg(rgb(0xffffff))
                .style()
                .clone(),
        ),
        interactivity: Default::default(),
        children: vec![
            wire::Node::Image {
                id: Some(wire::ElementIdWire::Name("raster".into())),
                hash: 11,
                data: with_bytes.then(|| wire::ImageData::Rgba {
                    width: 1,
                    height: 1,
                    pixels: RED.to_vec(),
                }),
                label: None,
                image_style: wire::ImageStyle {
                    grayscale: false,
                    object_fit: wire::ImageObjectFit::Fill,
                },
                loading: false,
                fallback: false,
                state_children: Vec::new(),
                style: crate::render::test_style(image_style.style().clone()),
                interactivity: Default::default(),
            },
            wire::Node::Svg {
                id: Some(wire::ElementIdWire::Name("vector".into())),
                source: wire::SvgSource::Data {
                    hash: 22,
                    bytes: with_bytes.then(|| PRESENTATION_SVG.to_vec()),
                },
                transformation: wire::SvgTransformation {
                    scale: [1.0, 1.0],
                    translate: [0.0, 0.0],
                    rotate: 0.0,
                },
                label: None,
                style: crate::render::test_style(svg_style.style().clone()),
                interactivity: Default::default(),
            },
        ],
    })
}

/// A distinct one-pixel raster per `seed`.
fn pixel(seed: u64) -> wire::ImageData {
    let [a, b, c, ..] = seed.to_le_bytes();
    wire::ImageData::Rgba {
        width: 1,
        height: 1,
        pixels: vec![a, b, c, 255],
    }
}

/// The bytes a seat holds for `pictures(false)`'s two hashes.
fn held() -> Arc<PictureBytes> {
    let mut held = PictureBytes::default();
    held.raster.insert(
        11,
        Arc::new(wire::ImageData::Rgba {
            width: 1,
            height: 1,
            pixels: RED.to_vec(),
        }),
    );
    held.vector.insert(22, Arc::from(PRESENTATION_SVG));
    Arc::new(held)
}

fn drawn(
    tree: ViewTree,
    cx: &mut gpui_kit::TestAppContext,
) -> (Entity<Host>, gpui_kit::VisualTestContext) {
    let window = cx.open_window(size(px(80.), px(40.)), |_, cx| Host {
        tree: cx.new(|_| tree),
    });
    let host = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    native.update(|window, cx| window.render_frame(cx));
    (host, native)
}

fn svg_admitted(native: &mut gpui_kit::VisualTestContext) -> u64 {
    native
        .update(|window, cx| svg_limits::svg_admitted_bytes(window.window_handle().window_id(), cx))
}

/// A tree names a picture by hash alone once its bytes crossed: it draws
/// the seat's bytes under that hash, and nothing without them.
#[gpui_kit::test]
fn a_hash_only_picture_draws_from_the_seats_bytes(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let (_, mut native) = drawn(ViewTree::new(pictures(false)), cx);
    assert_eq!(svg_admitted(&mut native), 0, "no bytes, no SVG paint");

    let mut tree = ViewTree::new(pictures(false));
    tree.set_pictures(held());
    let (host, mut native) = drawn(tree, cx);
    assert!(
        svg_admitted(&mut native) > 0,
        "the SVG drew the seat's bytes"
    );
    let tree = host.read_with(&native, |host, _| host.tree.clone());
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.image_for_test(11).is_some(),
            "the raster decoded the seat's bytes"
        );
    });
}

#[gpui_kit::test]
fn picture_caches_survive_view_tree_recreation_without_resetting_limits(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let (host, mut native) = drawn(ViewTree::new(pictures(true)), cx);
    let admitted_before = svg_admitted(&mut native);
    assert!(admitted_before > 0, "the first SVG paint was admitted");

    let original = host.read_with(&native, |host, _| host.tree.clone());
    native.update(|_, cx| {
        original.update(cx, |tree, cx| {
            assert!(tree.image_for_test(11).is_some(), "decoded raster");
            // the cache's other 4,095 rasters, all drawn by this frame
            for hash in 100..100 + 4_095 {
                assert!(tree.remember_image(hash, &pixel(hash), None, cx));
            }
            assert!(tree.image_for_test(100 + 4_094).is_some());
        });
    });

    native.update(|window, cx| {
        let saved = original.read_with(cx, |tree, cx| tree.presentation(window, cx));
        host.update(cx, |host, cx| {
            host.tree = cx.new(|_| {
                let mut tree = ViewTree::new(pictures(false)).with_presentation(saved);
                tree.set_pictures(held());
                tree
            });
            cx.notify();
        });
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    native.update(|window, cx| window.render_frame(cx));
    let admitted_after = svg_admitted(&mut native);
    assert_eq!(
        admitted_after, admitted_before,
        "ViewTree recreation reused the native SVG admission key"
    );

    let replacement = host.read_with(&native, |host, _| host.tree.clone());
    native.update(|_, cx| {
        replacement.update(cx, |tree, cx| {
            assert!(
                tree.image_frame(11, None).is_some(),
                "hash-only raster resolves"
            );

            assert!(!tree.remember_image(9_999, &pixel(9_999), None, cx));
            assert!(
                tree.image_for_test(9_999).is_none(),
                "raster limit survived transfer"
            );
        });
    });
}

/// An Image with a loading child and a fallback child, each identified, so
/// a test reads which one the tree mounted.
fn image_with_states(hash: u64) -> wire::Node {
    let state = |name: &str| {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name(name.into())),
            style: crate::render::test_style(div().size(px(10.)).style().clone()),
            interactivity: Default::default(),
            children: Vec::new(),
        })
    };
    wire::Node::Image {
        id: Some(wire::ElementIdWire::Name("picture".into())),
        hash,
        data: Some(pixel(hash)),
        label: None,
        image_style: wire::ImageStyle {
            grayscale: false,
            object_fit: wire::ImageObjectFit::Fill,
        },
        loading: true,
        fallback: true,
        state_children: vec![state("loading"), state("fallback")],
        style: crate::render::test_style(div().size(px(30.)).style().clone()),
        interactivity: Default::default(),
    }
}

/// Whether the last frame drew the picture's state child `name`.
fn drew(native: &mut gpui_kit::VisualTestContext, name: &'static str) -> bool {
    native.update(|window, _| window.try_find(name).is_some())
}

/// A picture the raster cache has no room for, beside what the current
/// frame draws, shows the guest's fallback, not its loading placeholder
/// for good; once a new frame lands, the rasters the last one drew are
/// evicted and it draws.
#[gpui_kit::test]
fn a_full_raster_cache_draws_the_fallback_then_evicts(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let (host, mut native) = drawn(ViewTree::new(image_with_states(7)), cx);
    let tree = host.read_with(&native, |host, _| host.tree.clone());
    // app-level, as the seat updates its tree, so no window is leased
    native.cx.update(|cx| {
        tree.update(cx, |tree, cx| {
            // the decoded picture leaves; the cache fills with rasters this
            // frame drew
            tree.release(cx);
            for hash in 100..100 + 4_096 {
                assert!(tree.remember_image(hash, &pixel(hash), None, cx));
            }
            cx.notify();
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert!(tree.image_for_test(7).is_none(), "the cache had no room");
    });
    assert!(
        drew(&mut native, "fallback"),
        "the full cache drew the fallback"
    );
    assert!(!drew(&mut native, "loading"), "not the loading placeholder");

    native
        .cx
        .update(|cx| tree.update(cx, |tree, cx| tree.replace(image_with_states(7), &[], cx)));
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert!(tree.image_for_test(7).is_some(), "the picture drew");
        let evicted = (100..100 + 4_096)
            .filter(|hash| tree.image_for_test(*hash).is_none())
            .count();
        assert_eq!(evicted, 1, "one raster the last frame drew made room");
    });
    assert!(!drew(&mut native, "fallback") && !drew(&mut native, "loading"));
}

/// A seat that drops releases its pictures' atlas tiles; a hot swap, whose
/// next tree draws on with them, releases nothing.
#[gpui_kit::test]
fn a_released_tree_drops_its_atlas_tiles_and_a_hot_swap_keeps_them(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let (host, mut native) = drawn(ViewTree::new(pictures(true)), cx);
    let original = host.read_with(&native, |host, _| host.tree.clone());
    let image = original
        .read_with(&native, |tree, _| tree.image_for_test(11))
        .expect("decoded raster");
    assert!(native.update(|window, _| window.has_image_atlas_entry(&image)));

    native.update(|window, cx| {
        let saved = original.read_with(cx, |tree, cx| tree.presentation(window, cx));
        host.update(cx, |host, cx| {
            host.tree = cx.new(|_| ViewTree::new(pictures(false)).with_presentation(saved));
            cx.notify();
        });
    });
    drop(original);
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    assert!(
        native.update(|window, _| window.has_image_atlas_entry(&image)),
        "the hot swap kept the tile"
    );

    let replacement = host.read_with(&native, |host, _| host.tree.clone());
    // app-level, as `Seats::reconcile` releases it
    native
        .cx
        .update(|cx| replacement.update(cx, |tree, cx| tree.release(cx)));
    assert!(
        !native.update(|window, _| window.has_image_atlas_entry(&image)),
        "the released tree's tile left the atlas"
    );
}

/// The raster cache's cost, measured: a seat drawing 64 pictures of 1 MiB
/// decoded (the byte cap, every one drawn), then 64 fresh ones that
/// displace them. The seat's raw bytes (P16's store) are built before the
/// first reading. gpui's test atlas keeps no pixels: a GPU holds each
/// drawn raster again as its atlas tile. Run alone: `cargo test
/// a_seat_at_its_raster_cap -- --ignored --exact --nocapture`.
#[gpui_kit::test]
#[ignore = "measurement: prints resident memory, asserts nothing of it"]
fn a_seat_at_its_raster_cap(cx: &mut gpui_kit::TestAppContext) {
    const SIDE: u32 = 512;
    cx.update(gpui_kit::init);
    let held = |from: u64| {
        let mut held = PictureBytes::default();
        for hash in from..from + 64 {
            let pixels = vec![hash as u8; (SIDE * SIDE * 4) as usize];
            held.raster.insert(
                hash,
                Arc::new(wire::ImageData::Rgba {
                    width: SIDE,
                    height: SIDE,
                    pixels,
                }),
            );
        }
        Arc::new(held)
    };
    let row = |from: u64| {
        let mut row = image_with_states(0);
        let wire::Node::Image { data, .. } = &mut row else {
            unreachable!()
        };
        *data = None;
        wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: crate::render::test_style(div().flex().flex_wrap().size_full().style().clone()),
            interactivity: Default::default(),
            children: (from..from + 64)
                .map(|hash| {
                    let mut image = row.clone();
                    if let wire::Node::Image { hash: named, .. } = &mut image {
                        *named = hash;
                    }
                    image
                })
                .collect(),
        })
    };
    let first = held(0);
    let before = resident_mib();
    let mut tree = ViewTree::new(row(0));
    tree.set_pictures(first);
    let (host, mut native) = drawn(tree, cx);
    let filled = resident_mib();
    let tree = host.read_with(&native, |host, _| host.tree.clone());
    let decoded = |native: &mut gpui_kit::VisualTestContext, from: u64| {
        tree.read_with(native, |tree, _| {
            (from..from + 64)
                .filter(|hash| tree.image_for_test(*hash).is_some())
                .count()
        })
    };
    let first_decoded = decoded(&mut native, 0);
    let second = held(64);
    native.cx.update(|cx| {
        tree.update(cx, |tree, cx| {
            tree.set_pictures(second);
            tree.replace(row(64), &[], cx);
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    let churned = resident_mib();
    println!(
        "64 rasters of {} B: {first_decoded} decoded, resident +{:.1} MiB; 64 fresh ones: {} decoded, {} of the first left, resident +{:.1} MiB",
        SIDE * SIDE * 4,
        filled - before,
        decoded(&mut native, 64),
        decoded(&mut native, 0),
        churned - before,
    );
}
