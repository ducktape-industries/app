use super::*;

// Native picture caches belong to presentation state, not one ViewTree entity.
const PRESENTATION_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><path d="M0 0h1v1H0z"/></svg>"#;

#[gpui_kit::test]
fn picture_caches_survive_view_tree_recreation_without_resetting_limits(
    cx: &mut gpui_kit::TestAppContext,
) {
    struct Host {
        tree: Entity<ViewTree>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.tree.clone())
        }
    }

    fn pictures(with_bytes: bool) -> wire::Node {
        let mut image_style = div().size(px(30.));
        let mut svg_style = div().size(px(30.)).text_color(rgb(0x00ff00));
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("pictures".into())),
            style: div()
                .flex()
                .flex_row()
                .size_full()
                .bg(rgb(0xffffff))
                .style()
                .clone(),
            interactivity: Default::default(),
            children: vec![
                wire::Node::Image {
                    id: Some(wire::ElementIdWire::Name("raster".into())),
                    hash: 11,
                    data: with_bytes.then(|| wire::ImageData::Rgba {
                        width: 1,
                        height: 1,
                        pixels: vec![255, 0, 0, 255],
                    }),
                    label: None,
                    image_style: wire::ImageStyle {
                        grayscale: false,
                        object_fit: wire::ImageObjectFit::Fill,
                    },
                    loading: false,
                    fallback: false,
                    state_children: Vec::new(),
                    style: image_style.style().clone(),
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
                    style: svg_style.style().clone(),
                    interactivity: Default::default(),
                },
            ],
        })
    }

    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(80.), px(40.)), |_, cx| Host {
        tree: cx.new(|_| ViewTree::new(pictures(true))),
    });
    let host = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    native.update(|window, cx| window.render_frame(cx));
    let admitted_before = native.update(|window, cx| {
        svg_limits::svg_admitted_bytes(window.window_handle().window_id(), cx)
    });
    assert!(admitted_before > 0, "the first SVG paint was admitted");

    let original = host.read_with(&native, |host, _| host.tree.clone());
    native.update(|_, cx| {
        original.update(cx, |tree, _| {
            let image = tree.images.get(&11).expect("decoded raster").clone();
            let vector = tree.vectors.get(&22).expect("retained SVG").clone();
            for hash in 100..100 + 4_095 {
                tree.images.insert(hash, image.clone());
                tree.vectors.insert(hash, vector.clone());
            }
            assert_eq!(tree.images.len(), 4_096);
            assert_eq!(tree.vectors.len(), 4_096);
        });
    });

    native.update(|window, cx| {
        let saved = original.read_with(cx, |tree, cx| tree.presentation(window, cx));
        host.update(cx, |host, cx| {
            host.tree = cx.new(|_| ViewTree::new(pictures(false)).with_presentation(saved));
            cx.notify();
        });
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    native.update(|window, cx| window.render_frame(cx));
    let admitted_after = native.update(|window, cx| {
        svg_limits::svg_admitted_bytes(window.window_handle().window_id(), cx)
    });
    assert_eq!(
        admitted_after, admitted_before,
        "ViewTree recreation reused the native SVG admission key"
    );

    let replacement = host.read_with(&native, |host, _| host.tree.clone());
    native.update(|_, cx| {
        replacement.update(cx, |tree, _| {
            assert!(
                tree.image_frame(11, None).is_some(),
                "hash-only raster resolves"
            );
            assert_eq!(
                tree.vectors.get(&22).map(|bytes| bytes.as_ref()),
                Some(PRESENTATION_SVG)
            );

            tree.remember_image(
                9_999,
                &wire::ImageData::Rgba {
                    width: 1,
                    height: 1,
                    pixels: vec![0, 0, 0, 255],
                },
            );
            tree.remember_vector(9_999, b"<svg/>");
            assert!(
                !tree.images.contains_key(&9_999),
                "raster limit survived transfer"
            );
            assert!(
                !tree.vectors.contains_key(&9_999),
                "vector limit survived transfer"
            );
        });
    });
}
